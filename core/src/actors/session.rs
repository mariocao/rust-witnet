use log::warn;
use std::io::Error;
use std::net::SocketAddr;
use std::time::Duration;

use actix::io::{FramedWrite, WriteHandler};
use actix::{
    Actor, ActorContext, ActorFuture, AsyncContext, Context, ContextFutureSpawner, Handler,
    Message, Running, StreamHandler, System, WrapFuture,
};
use log::{debug, info};
use tokio::io::WriteHalf;
use tokio::net::TcpStream;

use crate::actors::codec::{P2PCodec, Request, Response};
use crate::actors::peers_manager;
use crate::actors::sessions_manager::{Consolidate, Register, SessionsManager, Unregister};

use witnet_data_structures::{
    serializers::TryFrom,
    types::{Address, Command, Message as WitnetMessage},
};
use witnet_p2p::sessions::{SessionStatus, SessionType};

////////////////////////////////////////////////////////////////////////////////////////
// ACTOR BASIC STRUCTURE
////////////////////////////////////////////////////////////////////////////////////////
/// Handshake flags
#[derive(Default)]
struct HandshakeFlags {
    /// Flag to indicate that a version message was tx
    version_tx: bool,
    /// Flag to indicate that a version message was rx
    version_rx: bool,
    /// Flag to indicate that a verack message was tx
    verack_tx: bool,
    /// Flag to indicate that a verack message was rx
    verack_rx: bool,
}

impl HandshakeFlags {
    pub fn all_true(&self) -> bool {
        self.version_tx && self.version_rx && self.verack_tx && self.verack_rx
    }
}

/// Session representing a TCP connection
pub struct Session {
    /// Server socket address (local peer)
    server_addr: SocketAddr,

    /// Remote socket address (remote server address if outbound)
    remote_addr: SocketAddr,

    /// Session type
    session_type: SessionType,

    /// Session status
    status: SessionStatus,

    /// Framed wrapper to send messages through the TCP connection
    framed: FramedWrite<WriteHalf<TcpStream>, P2PCodec>,

    /// Handshake timeout
    handshake_timeout: Duration,
    handshake_flags: HandshakeFlags,
}

/// Session helper methods
impl Session {
    /// Method to create a new session
    pub fn new(
        server_addr: SocketAddr,
        remote_addr: SocketAddr,
        session_type: SessionType,
        framed: FramedWrite<WriteHalf<TcpStream>, P2PCodec>,
        handshake_timeout: Duration,
    ) -> Session {
        Session {
            server_addr,
            remote_addr,
            session_type,
            status: SessionStatus::Unconsolidated,
            framed,
            handshake_timeout,
            handshake_flags: HandshakeFlags::default(),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////////////
// ACTOR IMPL
////////////////////////////////////////////////////////////////////////////////////////
/// Implement actor trait for Session
impl Actor for Session {
    /// Every actor has to provide execution `Context` in which it can run.
    type Context = Context<Self>;

    /// Method to be executed when the actor is started
    fn started(&mut self, ctx: &mut Self::Context) {
        // Set Handshake timeout for stopping actor if session is unconsolidated after given period of time
        ctx.run_later(self.handshake_timeout, |act, ctx| {
            if act.status != SessionStatus::Consolidated {
                info!(
                    "Handshake timeout expired, disconnecting session with peer {:?}",
                    act.remote_addr
                );
                if let SessionStatus::Unconsolidated = act.status {
                    ctx.stop();
                }
            }
        });

        // Get sessions manager address
        let sessions_manager_addr = System::current().registry().get::<SessionsManager>();

        // Register self in session manager. `AsyncContext::wait` register
        // future within context, but context waits until this future resolves
        // before processing any other events.
        sessions_manager_addr
            .send(Register {
                address: self.remote_addr,
                actor: ctx.address(),
                session_type: self.session_type,
            })
            .into_actor(self)
            .then(|res, _act, ctx| {
                match res {
                    Ok(Ok(_)) => {
                        debug!("Session successfully registered into the Session Manager");

                        actix::fut::ok(())
                    }
                    _ => {
                        debug!("Session register into Session Manager failed");
                        // FIXME(#72): a full stop of the session is not correct (unregister should
                        // be skipped)
                        ctx.stop();

                        actix::fut::err(())
                    }
                }
            })
            .and_then(|_, act, _ctx| {
                // Send version if outbound session
                if let SessionType::Outbound = act.session_type {
                    // ctx.notify(SendVersion);
                    // send_version(act);
                    let version: Vec<u8> =
                        WitnetMessage::build_version(act.server_addr, act.remote_addr, 0).into();
                    act.framed.write(Response(version.into()));
                }

                actix::fut::ok(())
            })
            .wait(ctx);
    }

    /// Method to be executed when the actor is stopping
    fn stopping(&mut self, _: &mut Self::Context) -> Running {
        // Get session manager address
        let session_manager_addr = System::current().registry().get::<SessionsManager>();

        // Unregister session from session manager
        session_manager_addr.do_send(Unregister {
            address: self.remote_addr,
            session_type: self.session_type,
            status: self.status,
        });

        Running::Stop
    }
}

////////////////////////////////////////////////////////////////////////////////////////
// ACTOR MESSAGES
////////////////////////////////////////////////////////////////////////////////////////
/// Message result of unit
pub type SessionUnitResult = ();

/// Message to indicate that the session needs to send a GetPeers message through the network
pub struct GetPeers;

impl Message for GetPeers {
    type Result = SessionUnitResult;
}

/// Message to indicate that the Version message have been received
pub struct Version {
    _msg: WitnetMessage,
}

impl Message for Version {
    type Result = SessionUnitResult;
}

/// Message to indicate that the Verack message have been received
pub struct Verack;

impl Message for Verack {
    type Result = SessionUnitResult;
}

////////////////////////////////////////////////////////////////////////////////////////
// ACTOR MESSAGE HANDLERS
////////////////////////////////////////////////////////////////////////////////////////
/// Implement `WriteHandler` for Session
impl WriteHandler<Error> for Session {}

/// Implement `StreamHandler` trait in order to use `Framed` with an actor
impl StreamHandler<Request, Error> for Session {
    /// This is main event loop for client requests
    fn handle(&mut self, req: Request, ctx: &mut Self::Context) {
        let Request(bytes) = req;
        info!("Session {} received message: {:?}", self.remote_addr, bytes);
        let result = WitnetMessage::try_from(bytes.to_vec());
        match result {
            Err(err) => debug!("Error decoding message: {:?}", err),
            Ok(msg) => {
                match (self.status, msg.kind) {
                    (SessionStatus::Unconsolidated, Command::Version { .. }) => {
                        let resps = handshake_version(self);
                        for response in resps {
                            self.framed.write(response);
                        }
                        try_consolidate_session(self, ctx);
                    }
                    (SessionStatus::Unconsolidated, Command::Verack) => {
                        handshake_verack(self);
                        try_consolidate_session(self, ctx);
                    }
                    (SessionStatus::Consolidated, Command::GetPeers) => {
                        peer_discovery_get_peers(self, ctx);
                    }
                    (SessionStatus::Consolidated, Command::Peers { peers }) => {
                        peer_discovery_peers(peers);
                    }
                    // TODO: Handle not implemented commands
                    (SessionStatus::Consolidated, _) => {
                        debug!("Not implemented message command received!");
                    }
                    (_, kind) => {
                        warn!(
                            "Received a message of kind \"{:?}\", which is not implemented yet",
                            kind
                        );
                    }
                };
            }
        }
    }
}

/// Handler for GetPeers message.
impl Handler<GetPeers> for Session {
    type Result = SessionUnitResult;

    fn handle(&mut self, _msg: GetPeers, _: &mut Context<Self>) {
        debug!("GetPeers message should be sent through the network");
        // Create get peers message
        let get_peers_msg: Vec<u8> = WitnetMessage::build_get_peers().into();
        // Write get peers message in session
        self.framed.write(Response(get_peers_msg.into()));
    }
}

fn handshake_version(session: &mut Session) -> Vec<Response> {
    info!("Version message have been received");
    let flags = &mut session.handshake_flags;

    if flags.version_rx {
        debug!("Version message already received");
        // TODO: placeholder to change behaviour (right only logging)
    }

    // Set version_rx flag
    flags.version_rx = true;

    let mut responses: Vec<Response> = vec![];
    if !flags.version_tx {
        flags.version_tx = true;
        let version: Vec<u8> =
            WitnetMessage::build_version(session.server_addr, session.remote_addr, 0).into();
        responses.push(Response(version.into()));
    }

    if !flags.verack_tx {
        flags.verack_tx = true;
        let verack: Vec<u8> = WitnetMessage::build_verack().into();
        responses.push(Response(verack.into()));
    }

    responses
}

fn handshake_verack(session: &mut Session) {
    debug!("Verack message have been received through the network");
    let flags = &mut session.handshake_flags;

    if flags.verack_rx {
        debug!("Verack message already received");
        // TODO: placeholder to change behaviour (right only logging)
    }

    // Set verack_rx flag
    flags.verack_rx = true;
}

fn try_consolidate_session(session: &mut Session, ctx: &mut Context<Session>) {
    // Check if version message was already sent
    if session.handshake_flags.all_true() {
        session.status = SessionStatus::Consolidated;
        // Notify the SessionsManager that this session has been consolidated
        update_consolidate(session, ctx);
    }
}

fn update_consolidate(session: &Session, ctx: &mut Context<Session>) {
    // Get session manager address
    let session_manager_addr = System::current().registry().get::<SessionsManager>();

    // Register self in session manager. `AsyncContext::wait` register
    // future within context, but context waits until this future resolves
    // before processing any other events.
    session_manager_addr
        .send(Consolidate {
            address: session.remote_addr,
            //TODO: Check address
            potential_new_peer: session.remote_addr,
            session_type: session.session_type,
        })
        .into_actor(session)
        .then(|res, _act, ctx| {
            match res {
                Ok(Ok(_)) => {
                    debug!("Session successfully consolidated in the Session Manager");

                    actix::fut::ok(())
                }
                _ => {
                    debug!("Session consolidate in Session Manager failed");
                    // FIXME(#72): a full stop of the session is not correct (unregister should
                    // be skipped)
                    ctx.stop();

                    actix::fut::err(())
                }
            }
        })
        .wait(ctx);
}

fn peer_discovery_get_peers(session: &mut Session, ctx: &mut Context<Session>) {
    // Get the address of PeersManager actor
    let peers_manager_addr = System::current()
        .registry()
        .get::<peers_manager::PeersManager>();

    // Start chain of actions
    peers_manager_addr
        // Send GetPeer message to PeersManager actor
        // This returns a Request Future, representing an asynchronous message sending process
        .send(peers_manager::GetPeers)
        // Convert a normal future into an ActorFuture
        .into_actor(session)
        // Process the response from PeersManager
        // This returns a FutureResult containing the socket address if present
        .then(|res, act, ctx| {
            match res {
                Ok(Ok(addresses)) => {
                    debug!("Get peers successfully registered into the Peers Manager");
                    let peers_msg: Vec<u8> = WitnetMessage::build_peers(&addresses).into();
                    act.framed.write(Response(peers_msg.into()));
                }
                _ => {
                    debug!("Get peers register into Peers Manager failed");
                    // FIXME(#72): a full stop of the session is not correct (unregister should
                    // be skipped)
                    ctx.stop();
                }
            }
            actix::fut::ok(())
        })
        .wait(ctx);
}

fn peer_discovery_peers(_peers: Vec<Address>) {
    // Get peers manager address
    let peers_manager_addr = System::current()
        .registry()
        .get::<peers_manager::PeersManager>();

    // Send AddPeers message to the peers manager
    peers_manager_addr.do_send(peers_manager::AddPeers {
        // TODO: convert Vec<Address> to Vec<SocketAddr>
        addresses: vec![],
    });
}
