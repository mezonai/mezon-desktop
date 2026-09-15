use std::any::Any;

use crate::impl_thread_safety;

#[cxx::bridge(namespace = "mezon_ffi")]
pub mod ffi {
    #[repr(i32)]
    pub enum PeerConnectionState {
        New,
        Connecting,
        Connected,
        Disconnected,
        Failed,
        Closed,
    }

    #[repr(i32)]
    pub enum SignalingState {
        Stable,
        HaveLocalOffer,
        HaveLocalPrAnswer,
        HaveRemoteOffer,
        HaveRemotePrAnswer,
        Closed,
    }

    #[repr(i32)]
    pub enum IceConnectionState {
        IceConnectionNew,
        IceConnectionChecking,
        IceConnectionConnected,
        IceConnectionCompleted,
        IceConnectionFailed,
        IceConnectionDisconnected,
        IceConnectionClosed,
        IceConnectionMax,
    }

    #[repr(i32)]
    pub enum IceGatheringState {
        IceGatheringNew,
        IceGatheringGathering,
        IceGatheringComplete,
    }

    #[repr(i32)]
    pub enum ContinualGatheringPolicy {
        GatherOnce,
        GatherContinually,
    }

    #[repr(i32)]
    pub enum IceTransportsType {
        None,
        Relay,
        NoHost,
        All,
    }

    pub struct RtcOfferAnswerOptions {
        offer_to_receive_video: i32,
        offer_to_receive_audio: i32,
        voice_activity_detection: bool,
        ice_restart: bool,
        use_rtp_mux: bool,
        raw_packetization_for_video: bool,
        num_simulcast_layers: i32,
        use_obsolete_sctp_sdp: bool,
    }

    pub struct IceServer {
        pub urls: Vec<String>,
        pub username: String,
        pub password: String,
    }

    pub struct RtcConfiguration {
        pub ice_servers: Vec<IceServer>,
        pub continual_gathering_policy: ContinualGatheringPolicy,
        pub ice_transport_type: IceTransportsType,
    }

    extern "C++" {
        include!("mezon_rtc/rtc_error.h");
        include!("mezon_rtc/helper.h");
        include!("mezon_rtc/candidate.h");
        include!("mezon_rtc/media_stream.h");
        include!("mezon_rtc/rtp_transceiver.h");
        include!("mezon_rtc/rtp_sender.h");
        include!("mezon_rtc/rtp_receiver.h");
        include!("mezon_rtc/data_channel.h");
        include!("mezon_rtc/jsep.h");
        include!("mezon_rtc/webrtc.h");

        type RtpSenderPtr = crate::helper::ffi::RtpSenderPtr;
        type RtpReceiverPtr = crate::helper::ffi::RtpReceiverPtr;
        type RtpTransceiverPtr = crate::helper::ffi::RtpTransceiverPtr;
        type RtcError = crate::rtc_error::ffi::RtcError;
        type Candidate = crate::candidate::ffi::Candidate;
        type IceCandidate = crate::jsep::ffi::IceCandidate;
        type DataChannel = crate::data_channel::ffi::DataChannel;
        type DataChannelInit = crate::data_channel::ffi::DataChannelInit;
        type RtpSender = crate::rtp_sender::ffi::RtpSender;
        type RtpReceiver = crate::rtp_receiver::ffi::RtpReceiver;
        type RtpTransceiver = crate::rtp_transceiver::ffi::RtpTransceiver;
        type RtpTransceiverInit = crate::rtp_transceiver::ffi::RtpTransceiverInit;
        type MediaStream = crate::media_stream::ffi::MediaStream;
        type MediaStreamTrack = crate::media_stream::ffi::MediaStreamTrack;
        type SessionDescription = crate::jsep::ffi::SessionDescription;
        type MediaType = crate::webrtc::ffi::MediaType;
    }

    unsafe extern "C++" {
        include!("mezon_rtc/peer_connection.h");

        type PeerConnection;

        fn set_configuration(self: &PeerConnection, config: RtcConfiguration) -> Result<()>;

        fn create_offer(
            self: &PeerConnection,
            options: RtcOfferAnswerOptions,
            ctx: Box<PeerContext>,
            on_success: fn(ctx: Box<PeerContext>, sdp: UniquePtr<SessionDescription>),
            on_error: fn(ctx: Box<PeerContext>, error: RtcError),
        );
        fn create_answer(
            self: &PeerConnection,
            options: RtcOfferAnswerOptions,
            ctx: Box<PeerContext>,
            on_success: fn(ctx: Box<PeerContext>, sdp: UniquePtr<SessionDescription>),
            on_error: fn(ctx: Box<PeerContext>, error: RtcError),
        );
        fn set_local_description(
            self: &PeerConnection,
            desc: UniquePtr<SessionDescription>,
            ctx: Box<PeerContext>,
            on_complete: fn(ctx: Box<PeerContext>, error: RtcError),
        );
        fn set_remote_description(
            self: &PeerConnection,
            desc: UniquePtr<SessionDescription>,
            ctx: Box<PeerContext>,
            on_complete: fn(ctx: Box<PeerContext>, error: RtcError),
        );
        fn add_track(
            self: &PeerConnection,
            track: SharedPtr<MediaStreamTrack>,
            stream_ids: &Vec<String>,
        ) -> Result<SharedPtr<RtpSender>>;
        fn remove_track(self: &PeerConnection, sender: SharedPtr<RtpSender>) -> Result<()>;
        fn get_stats(
            self: &PeerConnection,
            ctx: Box<PeerContext>,
            on_stats: fn(ctx: Box<PeerContext>, json: String),
        );
        fn add_transceiver(
            self: &PeerConnection,
            track: SharedPtr<MediaStreamTrack>,
            init: RtpTransceiverInit,
        ) -> Result<SharedPtr<RtpTransceiver>>;
        fn add_transceiver_for_media(
            self: &PeerConnection,
            media_type: MediaType,
            init: RtpTransceiverInit,
        ) -> Result<SharedPtr<RtpTransceiver>>;
        fn get_senders(self: &PeerConnection) -> Vec<RtpSenderPtr>;
        fn get_receivers(self: &PeerConnection) -> Vec<RtpReceiverPtr>;
        fn get_transceivers(self: &PeerConnection) -> Vec<RtpTransceiverPtr>;
        fn create_data_channel(
            self: &PeerConnection,
            label: String,
            init: DataChannelInit,
        ) -> Result<SharedPtr<DataChannel>>;
        fn add_ice_candidate(
            self: &PeerConnection,
            candidate: SharedPtr<IceCandidate>,
            ctx: Box<PeerContext>,
            on_complete: fn(ctx: Box<PeerContext>, error: RtcError),
        );
        fn restart_ice(self: &PeerConnection);
        fn current_local_description(self: &PeerConnection) -> UniquePtr<SessionDescription>;
        fn current_remote_description(self: &PeerConnection) -> UniquePtr<SessionDescription>;
        fn connection_state(self: &PeerConnection) -> PeerConnectionState;
        fn signaling_state(self: &PeerConnection) -> SignalingState;
        fn ice_gathering_state(self: &PeerConnection) -> IceGatheringState;
        fn ice_connection_state(self: &PeerConnection) -> IceConnectionState;
        fn close(self: &PeerConnection);

        fn _shared_peer_connection() -> SharedPtr<PeerConnection>; // Ignore
    }

    extern "Rust" {
        type PeerContext;
    }
}

#[repr(transparent)]
pub struct PeerContext(pub Box<dyn Any + Send>);

// https://webrtc.github.io/webrtc-org/native-code/native-apis/
impl_thread_safety!(ffi::PeerConnection, Send + Sync);

impl Default for ffi::RtcOfferAnswerOptions {
    // static const int kUndefined = -1;
    // static const int kMaxOfferToReceiveMedia = 1;
    // static const int kOfferToReceiveMediaTrue = 1;

    fn default() -> Self {
        Self {
            offer_to_receive_video: -1,
            offer_to_receive_audio: -1,
            voice_activity_detection: true,
            ice_restart: false,
            use_rtp_mux: true,
            raw_packetization_for_video: false,
            num_simulcast_layers: 1,
            use_obsolete_sctp_sdp: false,
        }
    }
}
