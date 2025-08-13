use std::{
    sync::{Arc, RwLock},
    time::{Duration, SystemTime},
};

use jito_protos::auth::{
    GenerateAuthChallengeRequest, GenerateAuthTokensRequest, RefreshAccessTokenRequest, Role,
    Token, auth_service_client::AuthServiceClient,
};
use prost_types::Timestamp;
use solana_sdk::signature::{Keypair, Signer};
use tokio::{task::JoinHandle, time::sleep};
use tonic::{Request, service::Interceptor, transport::Channel};

use jito_protos::{
    bundle::Bundle,
    convert::proto_packet_from_versioned_tx,
    searcher::{
        SendBundleRequest, SendBundleResponse, searcher_service_client::SearcherServiceClient,
    },
};
use solana_sdk::transaction::VersionedTransaction;
use thiserror::Error;
use tonic::{Response, Status, codegen::InterceptedService, transport, transport::Endpoint};

#[derive(Debug, Error)]
pub enum BlockEngineConnectionError {
    #[error("transport error {0}")]
    TransportError(#[from] transport::Error),
    #[error("client error {0}")]
    ClientError(#[from] Status),
}

#[derive(Debug, Error)]
pub enum BundleRejectionError {
    #[error("bundle lost state auction, auction: {0}, tip {1} lamports")]
    StateAuctionBidRejected(String, u64),
    #[error("bundle won state auction but failed global auction, auction {0}, tip {1} lamports")]
    WinningBatchBidRejected(String, u64),
    #[error("bundle simulation failure on tx {0}, message: {1:?}")]
    SimulationFailure(String, Option<String>),
    #[error("internal error {0}")]
    InternalError(String),
}

pub type BlockEngineConnectionResult<T> = Result<T, BlockEngineConnectionError>;
pub type Client = SearcherServiceClient<InterceptedService<Channel, ClientInterceptor>>;

pub async fn get_searcher_client_auth(
    block_engine_url: &str,
    auth_keypair: &Arc<Keypair>,
) -> BlockEngineConnectionResult<Client> {
    let auth_channel = create_grpc_channel(block_engine_url).await?;
    println!("auth_channel: {:?}", auth_channel);
    let client_interceptor = ClientInterceptor::new(
        AuthServiceClient::new(auth_channel),
        auth_keypair,
        Role::Searcher,
    )
    .await?;

    let searcher_channel = create_grpc_channel(block_engine_url).await?;
    let searcher_client =
        SearcherServiceClient::with_interceptor(searcher_channel, client_interceptor);
    Ok(searcher_client)
}

pub async fn get_searcher_client_no_auth(
    block_engine_url: &str,
) -> BlockEngineConnectionResult<SearcherServiceClient<Channel>> {
    let searcher_channel = create_grpc_channel(block_engine_url).await?;
    let searcher_client = SearcherServiceClient::new(searcher_channel);
    Ok(searcher_client)
}

pub async fn create_grpc_channel(url: &str) -> BlockEngineConnectionResult<Channel> {
    let endpoint = Endpoint::from_shared(url.to_string())
        .expect("invalid url")
        .tls_config(tonic::transport::ClientTlsConfig::new().with_native_roots())
        .expect("failed to configure tls");
    Ok(endpoint.connect().await?)
}

pub async fn send_bundle_no_wait(
    transactions: &[VersionedTransaction],
    searcher_client: &mut Client,
) -> Result<Response<SendBundleResponse>, Status> {
    // convert them to packets + send over
    let packets: Vec<_> = transactions
        .iter()
        .map(proto_packet_from_versioned_tx)
        .collect();

    searcher_client
        .send_bundle(SendBundleRequest {
            bundle: Some(Bundle {
                header: None,
                packets,
            }),
        })
        .await
}

const AUTHORIZATION_HEADER: &str = "authorization";
const BEARER: &str = "Bearer ";

/// Adds the token to each requests' authorization header.
/// Manages refreshing the token in a separate thread.
#[derive(Clone)]
pub struct ClientInterceptor {
    /// The token added to each request header.
    bearer_token: Arc<RwLock<String>>,
}

impl ClientInterceptor {
    pub async fn new(
        mut auth_service_client: AuthServiceClient<Channel>,
        keypair: &Arc<Keypair>,
        role: Role,
    ) -> BlockEngineConnectionResult<Self> {
        let (access_token, refresh_token) =
            Self::auth(&mut auth_service_client, keypair, role).await?;

        let bearer_token = Arc::new(RwLock::new(access_token.value.clone()));

        let _refresh_token_thread = Self::spawn_token_refresh_thread(
            auth_service_client,
            bearer_token.clone(),
            refresh_token,
            access_token.expires_at_utc.unwrap(),
            keypair.clone(),
            role,
        );

        Ok(Self { bearer_token })
    }

    async fn auth(
        auth_service_client: &mut AuthServiceClient<Channel>,
        keypair: &Keypair,
        role: Role,
    ) -> BlockEngineConnectionResult<(Token, Token)> {
        let challenge_resp = auth_service_client
            .generate_auth_challenge(GenerateAuthChallengeRequest {
                role: role as i32,
                pubkey: keypair.pubkey().as_ref().to_vec(),
            })
            .await?
            .into_inner();
        let challenge = format!("{}-{}", keypair.pubkey(), challenge_resp.challenge);
        let signed_challenge = keypair.sign_message(challenge.as_bytes()).as_ref().to_vec();

        let tokens = auth_service_client
            .generate_auth_tokens(GenerateAuthTokensRequest {
                challenge,
                client_pubkey: keypair.pubkey().as_ref().to_vec(),
                signed_challenge,
            })
            .await?
            .into_inner();

        Ok((tokens.access_token.unwrap(), tokens.refresh_token.unwrap()))
    }

    fn spawn_token_refresh_thread(
        mut auth_service_client: AuthServiceClient<Channel>,
        bearer_token: Arc<RwLock<String>>,
        refresh_token: Token,
        access_token_expiration: Timestamp,
        keypair: Arc<Keypair>,
        role: Role,
    ) -> JoinHandle<BlockEngineConnectionResult<()>> {
        tokio::spawn(async move {
            let mut refresh_token = refresh_token;
            let mut access_token_expiration = access_token_expiration;

            loop {
                let access_token_ttl = SystemTime::try_from(access_token_expiration.clone())
                    .unwrap()
                    .duration_since(SystemTime::now())
                    .unwrap_or_else(|_| Duration::from_secs(0));
                let refresh_token_ttl =
                    SystemTime::try_from(refresh_token.expires_at_utc.as_ref().unwrap().clone())
                        .unwrap()
                        .duration_since(SystemTime::now())
                        .unwrap_or_else(|_| Duration::from_secs(0));

                let does_access_token_expire_soon = access_token_ttl < Duration::from_secs(5 * 60);
                let does_refresh_token_expire_soon =
                    refresh_token_ttl < Duration::from_secs(5 * 60);

                match (
                    does_refresh_token_expire_soon,
                    does_access_token_expire_soon,
                ) {
                    // re-run entire auth workflow is refresh token expiring soon
                    (true, _) => {
                        let is_error = {
                            if let Ok((new_access_token, new_refresh_token)) =
                                Self::auth(&mut auth_service_client, &keypair, role).await
                            {
                                *bearer_token.write().unwrap() = new_access_token.value.clone();
                                access_token_expiration = new_access_token.expires_at_utc.unwrap();
                                refresh_token = new_refresh_token;
                                false
                            } else {
                                true
                            }
                        };
                        println!("searcher-full-auth, is_error: {}", is_error);
                    }
                    // re-up the access token if it expires soon
                    (_, true) => {
                        let is_error = {
                            if let Ok(refresh_resp) = auth_service_client
                                .refresh_access_token(RefreshAccessTokenRequest {
                                    refresh_token: refresh_token.value.clone(),
                                })
                                .await
                            {
                                let access_token = refresh_resp.into_inner().access_token.unwrap();
                                *bearer_token.write().unwrap() = access_token.value.clone();
                                access_token_expiration = access_token.expires_at_utc.unwrap();
                                false
                            } else {
                                true
                            }
                        };

                        println!("searcher-refresh-auth, is_error: {}", is_error);
                    }
                    _ => {
                        sleep(Duration::from_secs(60)).await;
                    }
                }
            }
        })
    }
}

impl Interceptor for ClientInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        let l_token = self.bearer_token.read().unwrap();
        if !l_token.is_empty() {
            request.metadata_mut().insert(
                AUTHORIZATION_HEADER,
                format!("{BEARER}{l_token}").parse().unwrap(),
            );
        }

        Ok(request)
    }
}
