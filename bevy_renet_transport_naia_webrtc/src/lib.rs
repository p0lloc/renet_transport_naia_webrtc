use bevy_renet::renet::{transport::NetcodeTransportError, RenetClient};

use bevy::{app::AppExit, prelude::*};

use bevy_renet::RenetClientPlugin;
pub use renet_transport_naia_webrtc_client::NetcodeWebRtcClientTransport;

pub struct NetcodeWebRtcClientPlugin;

impl Plugin for NetcodeWebRtcClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<NetcodeTransportError>();

        app.add_systems(
            PreUpdate,
            Self::update_system
                .run_if(resource_exists::<NetcodeWebRtcClientTransport>())
                .run_if(resource_exists::<RenetClient>())
                .after(RenetClientPlugin::update_system),
        );
        app.add_systems(
            PostUpdate,
            (Self::send_packets, Self::disconnect_on_exit)
                .run_if(resource_exists::<NetcodeWebRtcClientTransport>())
                .run_if(resource_exists::<RenetClient>()),
        );
    }
}

impl NetcodeWebRtcClientPlugin {
    pub fn update_system(
        mut transport: ResMut<NetcodeWebRtcClientTransport>,
        mut client: ResMut<RenetClient>,
        time: Res<Time>,
        mut transport_errors: EventWriter<NetcodeTransportError>,
    ) {
        if let Err(e) = transport.update(time.delta(), &mut client) {
            transport_errors.send(e);
        }
    }

    pub fn send_packets(
        mut transport: ResMut<NetcodeWebRtcClientTransport>,
        mut client: ResMut<RenetClient>,
        mut transport_errors: EventWriter<NetcodeTransportError>,
    ) {
        if let Err(e) = transport.send_packets(&mut client) {
            transport_errors.send(e);
        }
    }

    fn disconnect_on_exit(
        exit: EventReader<AppExit>,
        mut transport: ResMut<NetcodeWebRtcClientTransport>,
    ) {
        if !exit.is_empty() && !transport.is_disconnected() {
            transport.disconnect();
        }
    }
}

pub fn client_connected() -> impl FnMut(Option<Res<NetcodeWebRtcClientTransport>>) -> bool {
    |transport| match transport {
        Some(transport) => transport.is_connected(),
        None => false,
    }
}

pub fn client_diconnected() -> impl FnMut(Option<Res<NetcodeWebRtcClientTransport>>) -> bool {
    |transport| match transport {
        Some(transport) => transport.is_disconnected(),
        None => true,
    }
}

pub fn client_connecting() -> impl FnMut(Option<Res<NetcodeWebRtcClientTransport>>) -> bool {
    |transport| match transport {
        Some(transport) => transport.is_connecting(),
        None => false,
    }
}

pub fn client_just_connected(
) -> impl FnMut(Local<bool>, Option<Res<NetcodeWebRtcClientTransport>>) -> bool {
    |mut last_connected: Local<bool>, transport| {
        let Some(transport) = transport else {
            return false;
        };

        let connected = transport.is_connected();
        let just_connected = !*last_connected && connected;
        *last_connected = connected;
        just_connected
    }
}

pub fn client_just_diconnected(
) -> impl FnMut(Local<bool>, Option<Res<NetcodeWebRtcClientTransport>>) -> bool {
    |mut last_disconnected: Local<bool>, transport| {
        let Some(transport) = transport else {
            return true;
        };

        let disconnected = transport.is_disconnected();
        let just_disconnected = !*last_disconnected && disconnected;
        *last_disconnected = disconnected;
        just_disconnected
    }
}
