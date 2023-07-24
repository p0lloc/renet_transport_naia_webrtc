use std::{net::SocketAddr, time::Duration};

use naia_client_socket::{
    shared::SocketConfig, NaiaClientSocketError, PacketReceiver, PacketSender, Socket,
};
use renet::{transport::NetcodeTransportError, RenetClient};
use renetcode::{
    ConnectToken, DisconnectReason, NetcodeClient, NetcodeError, NETCODE_KEY_BYTES,
    NETCODE_USER_DATA_BYTES,
};

pub use naia_client_socket;

/// Configuration to establish an secure or unsecure connection with the server.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ClientAuthentication {
    /// Establishes a safe connection with the server using the [crate::transport::ConnectToken].
    ///
    /// See also [crate::transport::ServerAuthentication::Secure]
    Secure { connect_token: ConnectToken },
    /// Establishes an unsafe connection with the server, useful for testing and prototyping.
    ///
    /// See also [crate::transport::ServerAuthentication::Unsecure]
    Unsecure {
        protocol_id: u64,
        client_id: u64,
        server_addr: SocketAddr,
        user_data: Option<[u8; NETCODE_USER_DATA_BYTES]>,
    },
}

#[cfg_attr(feature = "bevy", derive(bevy_ecs::system::Resource))]
pub struct NetcodeWebRtcClientTransport {
    netcode_client: NetcodeClient,
    packet_receiver: Box<dyn PacketReceiver>,
    packet_sender: Box<dyn PacketSender>,
}

/// Sends the specified packet with the specified packet sender.
/// Converts a potential Naia error into a NetcodeTransportError.
fn send_packet(sender: &Box<dyn PacketSender>, packet: &[u8]) -> Result<(), NetcodeTransportError> {
    if let Err(e) = sender.send(packet) {
        return Err(convert_error(e));
    }

    return Ok(());
}

/// Converts a NaiaClientSocketError into a NetcodeTransportError.
fn convert_error(error: NaiaClientSocketError) -> NetcodeTransportError {
    return NetcodeTransportError::IO(std::io::Error::new(std::io::ErrorKind::Other, error));
}

impl NetcodeWebRtcClientTransport {
    pub fn new(
        server_url: &str,
        socket_config: &SocketConfig,

        current_time: Duration,
        authentication: ClientAuthentication,
    ) -> Result<Self, NetcodeError> {
        let (packet_sender, packet_receiver) = Socket::connect(server_url, socket_config);
        let connect_token: ConnectToken = match authentication {
            ClientAuthentication::Unsecure {
                server_addr,
                protocol_id,
                client_id,
                user_data,
            } => ConnectToken::generate(
                current_time,
                protocol_id,
                300,
                client_id,
                15,
                vec![server_addr],
                user_data.as_ref(),
                &[0; NETCODE_KEY_BYTES],
            )?,
            ClientAuthentication::Secure { connect_token } => connect_token,
        };

        let netcode_client = NetcodeClient::new(current_time, connect_token);

        Ok(Self {
            packet_sender,
            packet_receiver,
            netcode_client,
        })
    }

    pub fn client_id(&self) -> u64 {
        self.netcode_client.client_id()
    }

    pub fn is_connecting(&self) -> bool {
        self.netcode_client.is_connecting()
    }

    pub fn is_connected(&self) -> bool {
        self.netcode_client.is_connected()
    }

    pub fn is_disconnected(&self) -> bool {
        self.netcode_client.is_disconnected()
    }

    /// Returns the duration since the client last received a packet.
    /// Usefull to detect timeouts.
    pub fn time_since_last_received_packet(&self) -> Duration {
        self.netcode_client.time_since_last_received_packet()
    }

    /// Disconnect the client from the transport layer.
    /// This sends the disconnect packet instantly, use this when closing/exiting games,
    /// should use [RenetClient::disconnect][crate::RenetClient::disconnect] otherwise.
    pub fn disconnect(&mut self) {
        if self.netcode_client.is_disconnected() {
            return;
        }

        match self.netcode_client.disconnect() {
            Ok((_, packet)) => {
                if let Err(e) = send_packet(&self.packet_sender, packet) {
                    log::error!("Failed to send disconnect packet: {e}");
                }
            }
            Err(e) => log::error!("Failed to generate disconnect packet: {e}"),
        }
    }

    /// If the client is disconnected, returns the reason.
    pub fn disconnect_reason(&self) -> Option<DisconnectReason> {
        self.netcode_client.disconnect_reason()
    }

    /// Send packets to the server.
    /// Should be called every tick
    pub fn send_packets(
        &mut self,
        connection: &mut RenetClient,
    ) -> Result<(), NetcodeTransportError> {
        if let Some(reason) = self.netcode_client.disconnect_reason() {
            return Err(NetcodeError::Disconnected(reason).into());
        }

        let packets = connection.get_packets_to_send();
        for packet in packets {
            let (_, payload) = self.netcode_client.generate_payload_packet(&packet)?;
            send_packet(&self.packet_sender, payload)?;
        }

        Ok(())
    }

    /// Advances the transport by the duration, and receive packets from the network.
    pub fn update(
        &mut self,
        duration: Duration,
        client: &mut RenetClient,
    ) -> Result<(), NetcodeTransportError> {
        if let Some(reason) = self.netcode_client.disconnect_reason() {
            // Mark the client as disconnected if an error occured in the transport layer
            if !client.is_disconnected() {
                client.disconnect_due_to_transport();
            }

            return Err(NetcodeError::Disconnected(reason).into());
        }

        if let Some(error) = client.disconnect_reason() {
            let (_, disconnect_packet) = self.netcode_client.disconnect()?;
            send_packet(&self.packet_sender, disconnect_packet)?;
            return Err(error.into());
        }

        loop {
            let mut packet = match self.packet_receiver.receive() {
                Ok(Some(packet)) => packet.to_vec(),
                Ok(None) => {
                    break;
                }
                Err(e) => {
                    return Err(NetcodeTransportError::IO(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e,
                    )));
                }
            };

            if let Some(payload) = self.netcode_client.process_packet(&mut packet) {
                client.process_packet(payload);
            }
        }

        if let Some((packet, _)) = self.netcode_client.update(duration) {
            send_packet(&self.packet_sender, packet)?;
        }

        Ok(())
    }
}
