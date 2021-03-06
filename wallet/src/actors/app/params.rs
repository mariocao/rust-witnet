use std::net::SocketAddr;
use std::time::Duration;

use witnet_net::client::tcp::JsonRpcClient;

use super::*;
use crate::actors;

pub struct Params {
    pub testnet: bool,
    pub worker: Addr<actors::Worker>,
    pub client: Addr<JsonRpcClient>,
    pub server_addr: SocketAddr,
    pub session_expires_in: Duration,
    pub requests_timeout: Duration,
}
