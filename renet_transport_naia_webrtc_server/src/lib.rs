use std::{net::SocketAddr, time::Duration};

use naia_server_socket::{
    shared::SocketConfig, NaiaServerSocketError, PacketReceiver, PacketSender, ServerAddrs, Socket,
};
use renet::{transport::NetcodeTransportError, ClientId, RenetServer};
use renetcode::{
    NetcodeServer, ServerConfig, ServerResult, NETCODE_KEY_BYTES, NETCODE_USER_DATA_BYTES,
};

pub use naia_server_socket;

/// Configuration to establish a secure or unsecure connection with the server.
#[derive(Debug)]
pub enum ServerAuthentication {
    /// Establishes a safe connection using a private key for encryption. The private key cannot be
    /// shared with the client. Connections are stablished using [crate::transport::ConnectToken].
    ///
    /// See also [ClientAuthentication::Secure][crate::transport::ClientAuthentication::Secure]
    Secure {
        private_key: [u8; NETCODE_KEY_BYTES],
    },
    /// Establishes unsafe connections with clients, useful for testing and prototyping.
    ///
    /// See also [ClientAuthentication::Unsecure][crate::transport::ClientAuthentication::Unsecure]
    Unsecure,
}

#[cfg_attr(feature = "bevy", derive(bevy_ecs::system::Resource))]
pub struct NetcodeWebRtcServerTransport {
    netcode_server: NetcodeServer,

    packet_receiver: Box<dyn PacketReceiver>,
    packet_sender: Box<dyn PacketSender>,
}

impl NetcodeWebRtcServerTransport {
    pub fn new(
        server_addresses: &ServerAddrs,
        server_config: ServerConfig,
    ) -> Result<Self, std::io::Error> {
        let (packet_sender, packet_receiver) =
            Socket::listen(&server_addresses, &SocketConfig::new(None, None));

        let netcode_server = NetcodeServer::new(server_config);

        Ok(Self {
            netcode_server,

            packet_sender,
            packet_receiver,
        })
    }

    pub fn addresses(&self) -> Vec<SocketAddr> {
        self.netcode_server.addresses()
    }

    pub fn max_clients(&self) -> usize {
        self.netcode_server.max_clients()
    }

    pub fn connected_clients(&self) -> usize {
        self.netcode_server.connected_clients()
    }

    pub fn user_data(&self, client_id: u64) -> Option<[u8; NETCODE_USER_DATA_BYTES]> {
        self.netcode_server.user_data(client_id)
    }

    /// Disconnects all connected clients.
    /// This sends the disconnect packet instantly, use this when closing/exiting games,
    /// should use [RenetServer::disconnect_all][crate::RenetServer::disconnect_all] otherwise.
    pub fn disconnect_all(&mut self, server: &mut RenetServer) {
        for client_id in self.netcode_server.clients_id() {
            let server_result = self.netcode_server.disconnect(client_id);
            handle_server_result(server_result, &self.packet_sender, server);
        }
    }

    /// Returns the duration since the connected client last received a packet.
    /// Usefull to detect users that are timing out.
    pub fn time_since_last_received_packet(&self, client_id: u64) -> Option<Duration> {
        self.netcode_server
            .time_since_last_received_packet(client_id)
    }

    /// Advances the transport by the duration, and receive packets from the network.
    pub fn update(
        &mut self,
        duration: Duration,
        server: &mut RenetServer,
    ) -> Result<(), NetcodeTransportError> {
        self.netcode_server.update(duration);

        loop {
            match self.packet_receiver.receive() {
                Ok(Some((addr, payload))) => {
                    let mut cloned = payload.to_vec();
                    let server_result = self.netcode_server.process_packet(addr, &mut cloned);
                    handle_server_result(server_result, &self.packet_sender, server);
                }
                Ok(None) => {
                    break;
                }
                Err(e) => return Err(convert_error(e)),
            };
        }

        for client_id in self.netcode_server.clients_id() {
            let server_result = self.netcode_server.update_client(client_id);
            handle_server_result(server_result, &self.packet_sender, server);
        }

        for disconnection_id in server.disconnections_id() {
            let server_result = self.netcode_server.disconnect(disconnection_id.raw());
            handle_server_result(server_result, &self.packet_sender, server);
        }

        Ok(())
    }

    /// Send packets to connected clients.
    pub fn send_packets(&mut self, server: &mut RenetServer) {
        'clients: for client_id in server.clients_id() {
            let packets = server.get_packets_to_send(client_id).unwrap();
            for packet in packets {
                match self
                    .netcode_server
                    .generate_payload_packet(client_id.raw(), &packet)
                {
                    Ok((addr, payload)) => {
                        if let Err(e) = send_packet(&self.packet_sender, payload, &addr) {
                            log::error!(
                                "Failed to send packet to client {client_id} ({addr}): {e}"
                            );
                            continue 'clients;
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to encrypt payload packet for client {client_id}: {e}");
                        continue 'clients;
                    }
                }
            }
        }
    }
}

fn handle_server_result(
    server_result: ServerResult,
    sender: &Box<dyn PacketSender>,
    reliable_server: &mut RenetServer,
) {
    let send_packet = |packet: &[u8], addr: SocketAddr| {
        if let Err(err) = send_packet(sender, packet, &addr) {
            log::error!("Failed to send packet to {addr}: {err}");
        }
    };

    match server_result {
        ServerResult::None => {}
        ServerResult::PacketToSend { payload, addr } => {
            send_packet(payload, addr);
        }
        ServerResult::Payload { client_id, payload } => {
            let client_id = ClientId::from_raw(client_id);
            if let Err(e) = reliable_server.process_packet_from(payload, client_id) {
                log::error!("Error while processing payload for {}: {}", client_id, e);
            }
        }
        ServerResult::ClientConnected {
            client_id,
            user_data: _,
            addr,
            payload,
        } => {
            let client_id = ClientId::from_raw(client_id);
            reliable_server.add_connection(client_id);
            send_packet(payload, addr);
        }
        ServerResult::ClientDisconnected {
            client_id,
            addr,
            payload,
        } => {
            let client_id = ClientId::from_raw(client_id);
            reliable_server.remove_connection(client_id);
            if let Some(payload) = payload {
                send_packet(payload, addr);
            }
        }
    }
}

/// Sends the specified packet with the specified packet sender.
/// Converts a potential Naia error into a NetcodeTransportError.
fn send_packet(
    sender: &Box<dyn PacketSender>,
    payload: &[u8],
    addr: &SocketAddr,
) -> Result<(), NetcodeTransportError> {
    if let Err(e) = sender.send(addr, payload) {
        return Err(convert_error(e));
    }

    return Ok(());
}

/// Converts a NaiaClientSocketError into a NetcodeTransportError.
fn convert_error(error: NaiaServerSocketError) -> NetcodeTransportError {
    return NetcodeTransportError::IO(std::io::Error::new(std::io::ErrorKind::Other, error));
}
