pub const SCREEN_AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const SCREEN_AUDIO_CHANNELS: u32 = 2;

#[cfg(target_os = "macos")]
pub use macos::{ScreenAudioCapture, start_screen_audio};

#[cfg(target_os = "linux")]
pub use linux::{ScreenAudioCapture, start_screen_audio};

#[cfg(target_os = "windows")]
pub use wasapi::{ScreenAudioCapture, start_screen_audio};

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub use fallback::{ScreenAudioCapture, start_screen_audio};

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod fallback {
    pub struct ScreenAudioCapture {
        pub rx: flume::Receiver<Vec<i16>>,
    }

    pub fn start_screen_audio() -> Result<ScreenAudioCapture, String> {
        Err("system audio capture is not supported on this platform".into())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::rc::{Rc, Weak};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, OnceLock};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use libloading::Library;
    use pipewire as pw;
    use pw::properties::properties;
    use pw::registry::GlobalObject;
    use pw::spa;
    use pw::spa::utils::dict::DictRef;
    use pw::types::ObjectType;

    use super::{SCREEN_AUDIO_CHANNELS, SCREEN_AUDIO_SAMPLE_RATE};

    const OUTPUT_STREAM_CLASS: &str = "Stream/Output/Audio";
    const INVALID_NODE_ID: u32 = u32::MAX;

    struct Terminate;

    struct CaptureState {
        format: spa::param::audio::AudioInfoRaw,
        tx: flume::Sender<Vec<i16>>,
    }

    struct PortInfo {
        node: u32,
        output: bool,
        channel: String,
    }

    struct NodeWatch {
        // Remove the listener before releasing its proxy.
        _listener: pw::node::NodeListener,
        _node: pw::node::Node,
        client: Option<u32>,
        own: Option<bool>,
        ready: bool,
    }

    struct ClientWatch {
        _listener: pw::client::ClientListener,
        _client: pw::client::Client,
        own: Option<bool>,
    }

    struct Graph {
        process_identity_fix: bool,
        own_pid: String,
        own_binary: Option<String>,
        core: pw::core::Core,
        own_node: Option<u32>,
        own_ports: HashMap<String, u32>,
        app_nodes: HashSet<u32>,
        nodes: HashMap<u32, NodeWatch>,
        clients: HashMap<u32, ClientWatch>,
        ports: HashMap<u32, PortInfo>,
        links: HashMap<u32, Vec<pw::link::Link>>,
    }

    impl Graph {
        fn new(core: pw::core::Core) -> Self {
            Self {
                // Resolve full process identity on both X11 and Wayland.
                // Retain the original X11 flag as a rollback alias.
                process_identity_fix: std::env::var("MEZON_PIPEWIRE_SCREEN_AUDIO_FIX")
                    .or_else(|_| std::env::var("MEZON_X11_SCREEN_AUDIO_FIX"))
                    .as_deref()
                    != Ok("0"),
                own_pid: std::process::id().to_string(),
                own_binary: std::env::current_exe().ok().and_then(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                }),
                core,
                own_node: None,
                own_ports: HashMap::new(),
                app_nodes: HashSet::new(),
                nodes: HashMap::new(),
                clients: HashMap::new(),
                ports: HashMap::new(),
                links: HashMap::new(),
            }
        }

        fn adopt_own_node(&mut self, node: u32) {
            if node == INVALID_NODE_ID || self.own_node == Some(node) {
                return;
            }
            self.own_node = Some(node);
            self.own_ports.clear();
            for (id, port) in &self.ports {
                if port.node == node && !port.output {
                    self.own_ports.insert(port.channel.clone(), *id);
                }
            }
            self.link_pending();
        }

        fn global_added(
            &mut self,
            registry: &pw::registry::Registry,
            global: &GlobalObject<&DictRef>,
            graph: Weak<RefCell<Self>>,
        ) {
            let Some(props) = global.props else { return };
            match global.type_ {
                ObjectType::Node => {
                    if props.get(*pw::keys::MEDIA_CLASS) != Some(OUTPUT_STREAM_CLASS) {
                        return;
                    }
                    if !self.process_identity_fix {
                        self.node_added(global.id, props);
                        return;
                    }
                    // Registry globals omit process identity. Wait for full node
                    // info before linking, or Mezon's own playback is captured.
                    let node: pw::node::Node = match registry.bind(global) {
                        Ok(node) => node,
                        Err(e) => {
                            tracing::warn!(id = global.id, "screen audio node bind failed: {e}");
                            return;
                        }
                    };
                    let id = global.id;
                    let listener = node
                        .add_listener_local()
                        .info(move |info| {
                            if let (Some(graph), Some(props)) = (graph.upgrade(), info.props()) {
                                graph.borrow_mut().node_added(id, props);
                            }
                        })
                        .register();
                    self.nodes.insert(
                        id,
                        NodeWatch {
                            _listener: listener,
                            _node: node,
                            client: props
                                .get(*pw::keys::CLIENT_ID)
                                .and_then(|id| id.parse().ok()),
                            own: None,
                            ready: false,
                        },
                    );
                }
                ObjectType::Client => {
                    if !self.process_identity_fix {
                        return;
                    }
                    let client: pw::client::Client = match registry.bind(global) {
                        Ok(client) => client,
                        Err(e) => {
                            tracing::warn!(id = global.id, "screen audio client bind failed: {e}");
                            return;
                        }
                    };
                    let id = global.id;
                    let listener = client
                        .add_listener_local()
                        .info(move |info| {
                            if let (Some(graph), Some(props)) = (graph.upgrade(), info.props()) {
                                graph.borrow_mut().client_info(id, props);
                            }
                        })
                        .register();
                    self.clients.insert(
                        id,
                        ClientWatch {
                            _listener: listener,
                            _client: client,
                            own: None,
                        },
                    );
                }
                ObjectType::Port => self.port_added(global.id, props),
                _ => {}
            }
        }

        fn owner(&self, props: &DictRef) -> Option<bool> {
            let pid = props
                .get(*pw::keys::APP_PROCESS_ID)
                .filter(|value| !value.is_empty());
            let binary = props
                .get(*pw::keys::APP_PROCESS_BINARY)
                .filter(|value| !value.is_empty());
            if pid == Some(self.own_pid.as_str())
                || (binary.is_some() && binary == self.own_binary.as_deref())
            {
                Some(true)
            } else if pid.is_some() || binary.is_some() {
                Some(false)
            } else {
                None
            }
        }

        fn client_info(&mut self, id: u32, props: &DictRef) {
            let own = self.owner(props);
            if let Some(client) = self.clients.get_mut(&id) {
                client.own = own;
            }
            let nodes: Vec<u32> = self
                .nodes
                .iter()
                .filter(|(_, node)| node.client == Some(id))
                .map(|(&id, _)| id)
                .collect();
            for node in nodes {
                self.refresh_node(node);
            }
        }

        fn node_added(&mut self, id: u32, props: &DictRef) {
            let own = self.owner(props);
            if !self.process_identity_fix {
                if own != Some(true) {
                    self.app_nodes.insert(id);
                    self.link_pending();
                }
                return;
            }
            if let Some(node) = self.nodes.get_mut(&id) {
                node.own = own;
                node.ready = true;
                if let Some(client) = props.get(*pw::keys::CLIENT_ID) {
                    node.client = client.parse().ok();
                }
            }
            self.refresh_node(id);
        }

        fn refresh_node(&mut self, id: u32) {
            let Some(node) = self.nodes.get(&id) else {
                return;
            };
            let client_own = node
                .client
                .and_then(|id| self.clients.get(&id))
                .and_then(|c| c.own);
            // A PulseAudio proxy client may be foreign even when its node is
            // Mezon. An own-process match from either source always wins.
            let allowed = node.ready
                && node.own != Some(true)
                && client_own != Some(true)
                && (node.own == Some(false) || client_own == Some(false));
            if allowed {
                if self.app_nodes.insert(id) {
                    tracing::info!(id, "screen audio external output allowed");
                }
            } else {
                self.app_nodes.remove(&id);
                for (&port, info) in &self.ports {
                    if info.node == id {
                        self.links.remove(&port);
                    }
                }
            }
            self.link_pending();
        }

        fn port_added(&mut self, id: u32, props: &DictRef) {
            let Some(node) = props
                .get(*pw::keys::NODE_ID)
                .and_then(|node| node.parse::<u32>().ok())
            else {
                return;
            };
            let output = props.get(*pw::keys::PORT_DIRECTION) == Some("out");
            let channel = props
                .get(*pw::keys::AUDIO_CHANNEL)
                .unwrap_or("MONO")
                .to_owned();
            if self.own_node == Some(node) && !output {
                self.own_ports.insert(channel.clone(), id);
            }
            self.ports.insert(
                id,
                PortInfo {
                    node,
                    output,
                    channel,
                },
            );
            self.link_pending();
        }

        fn global_removed(&mut self, id: u32) {
            self.nodes.remove(&id);
            if self.clients.remove(&id).is_some() {
                let nodes: Vec<u32> = self
                    .nodes
                    .iter()
                    .filter(|(_, node)| node.client == Some(id))
                    .map(|(&id, _)| id)
                    .collect();
                for node in nodes {
                    if let Some(watch) = self.nodes.get_mut(&node) {
                        watch.ready = false;
                    }
                    self.refresh_node(node);
                }
            }
            if self.own_node == Some(id) {
                self.own_node = None;
                self.own_ports.clear();
                self.links.clear();
            }
            if self.app_nodes.remove(&id) {
                let orphaned: Vec<u32> = self
                    .ports
                    .iter()
                    .filter(|(_, port)| port.node == id)
                    .map(|(port, _)| *port)
                    .collect();
                for port in orphaned {
                    self.links.remove(&port);
                }
            }
            self.ports.remove(&id);
            self.links.remove(&id);
            self.own_ports.retain(|_, port| *port != id);
        }

        fn link_pending(&mut self) {
            let (Some(left), Some(right)) = (
                self.own_ports.get("FL").copied(),
                self.own_ports.get("FR").copied(),
            ) else {
                return;
            };
            let pending: Vec<(u32, String)> = self
                .ports
                .iter()
                .filter(|(id, port)| {
                    port.output
                        && self.app_nodes.contains(&port.node)
                        && !self.links.contains_key(id)
                })
                .map(|(id, port)| (*id, port.channel.clone()))
                .collect();
            for (port, channel) in pending {
                let targets: Vec<u32> = match channel.chars().last() {
                    Some('L') => vec![left],
                    Some('R') => vec![right],
                    _ => vec![left, right],
                };
                let mut created = Vec::with_capacity(targets.len());
                for target in targets {
                    match self.core.create_object::<pw::link::Link>(
                        "link-factory",
                        &properties! {
                            *pw::keys::LINK_OUTPUT_PORT => port.to_string(),
                            *pw::keys::LINK_INPUT_PORT => target.to_string(),
                            *pw::keys::OBJECT_LINGER => "false",
                        },
                    ) {
                        Ok(link) => {
                            created.push(link);
                        }
                        Err(e) => tracing::warn!("screen audio link {port}->{target} failed: {e}"),
                    }
                }
                self.links.insert(port, created);
            }
        }
    }

    enum CaptureStop {
        PipeWire(pw::channel::Sender<Terminate>),
        Pulse(Arc<AtomicBool>),
    }

    pub struct ScreenAudioCapture {
        pub rx: flume::Receiver<Vec<i16>>,
        stop: Option<CaptureStop>,
        thread: Option<JoinHandle<()>>,
    }

    impl Drop for ScreenAudioCapture {
        fn drop(&mut self) {
            match self.stop.take() {
                Some(CaptureStop::PipeWire(terminate)) => {
                    let _ = terminate.send(Terminate);
                }
                Some(CaptureStop::Pulse(stop)) => stop.store(true, Ordering::Relaxed),
                None => {}
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    pub fn start_screen_audio() -> Result<ScreenAudioCapture, String> {
        if let Some(capture) = start_pulse_capture()? {
            return Ok(capture);
        }
        if !crate::pipewire_init::ensure_pipewire_stubs_armed() {
            return Err("PipeWire unavailable for system audio capture".into());
        }
        let (sample_tx, sample_rx) = flume::bounded::<Vec<i16>>(64);
        let (init_tx, init_rx) = flume::bounded::<Result<(), String>>(1);
        let (terminate_tx, terminate_rx) = pw::channel::channel::<Terminate>();

        let thread = std::thread::Builder::new()
            .name("mezon-screen-audio-pw".into())
            .spawn(move || {
                if let Err(e) = build_and_run(&sample_tx, &init_tx, terminate_rx) {
                    let _ = init_tx.try_send(Err(e));
                }
            })
            .map_err(|e| format!("failed to spawn screen audio thread: {e}"))?;

        match init_rx.recv() {
            Ok(Ok(())) => Ok(ScreenAudioCapture {
                rx: sample_rx,
                stop: Some(CaptureStop::PipeWire(terminate_tx)),
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err("screen audio thread exited during init".into())
            }
        }
    }

    fn build_and_run(
        sample_tx: &flume::Sender<Vec<i16>>,
        init_tx: &flume::Sender<Result<(), String>>,
        terminate_rx: pw::channel::Receiver<Terminate>,
    ) -> Result<(), String> {
        pw::init();

        let mainloop =
            pw::main_loop::MainLoop::new(None).map_err(|e| format!("pipewire main loop: {e}"))?;
        let context =
            pw::context::Context::new(&mainloop).map_err(|e| format!("pipewire context: {e}"))?;
        let core = context
            .connect(None)
            .map_err(|e| format!("pipewire connect: {e}"))?;

        let _terminate = terminate_rx.attach(mainloop.loop_(), {
            let mainloop = mainloop.clone();
            move |_| mainloop.quit()
        });

        let registry = Rc::new(
            core.get_registry()
                .map_err(|e| format!("pipewire registry: {e}"))?,
        );
        let graph = Rc::new(RefCell::new(Graph::new(core.clone())));

        let props = properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::NODE_NAME => "mezon-screen-audio",
            *pw::keys::NODE_AUTOCONNECT => "false",
        };

        let stream = pw::stream::Stream::new(&core, "mezon-screen-audio", props)
            .map_err(|e| format!("pipewire stream: {e}"))?;

        let state = CaptureState {
            format: spa::param::audio::AudioInfoRaw::new(),
            tx: sample_tx.clone(),
        };

        let _listener = stream
            .add_local_listener_with_user_data(state)
            .state_changed({
                let graph = graph.clone();
                move |stream, _, _, state| {
                    if matches!(
                        state,
                        pw::stream::StreamState::Paused | pw::stream::StreamState::Streaming
                    ) {
                        graph.borrow_mut().adopt_own_node(stream.node_id());
                    }
                }
            })
            .param_changed(|_, state, id, param| {
                let Some(param) = param else {
                    return;
                };
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param)
                else {
                    return;
                };
                if media_type != spa::param::format::MediaType::Audio
                    || media_subtype != spa::param::format::MediaSubtype::Raw
                {
                    return;
                }
                let _ = state.format.parse(param);
            })
            .process(|stream, state| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let datas = buffer.datas_mut();
                let Some(data) = datas.first_mut() else {
                    return;
                };
                let size = data.chunk().size() as usize;
                let Some(bytes) = data.data() else {
                    return;
                };
                let end = size.min(bytes.len());
                let mut samples = Vec::with_capacity(end / 2);
                for pair in bytes[..end].chunks_exact(2) {
                    samples.push(i16::from_le_bytes([pair[0], pair[1]]));
                }
                if !samples.is_empty() {
                    let _ = state.tx.try_send(samples);
                }
            })
            .register()
            .map_err(|e| format!("pipewire stream listener: {e}"))?;

        let _registry_listener = registry
            .add_listener_local()
            .global({
                let graph = graph.clone();
                let registry = registry.clone();
                move |global| {
                    graph
                        .borrow_mut()
                        .global_added(&registry, global, Rc::downgrade(&graph))
                }
            })
            .global_remove({
                let graph = graph.clone();
                move |id| graph.borrow_mut().global_removed(id)
            })
            .register();

        let mut audio_info = spa::param::audio::AudioInfoRaw::new();
        audio_info.set_format(spa::param::audio::AudioFormat::S16LE);
        audio_info.set_rate(SCREEN_AUDIO_SAMPLE_RATE);
        audio_info.set_channels(SCREEN_AUDIO_CHANNELS);
        let obj = spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: audio_info.into(),
        };
        let values = spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &spa::pod::Value::Object(obj),
        )
        .map_err(|e| format!("pipewire format pod: {e:?}"))?
        .0
        .into_inner();
        let mut params = [spa::pod::Pod::from_bytes(&values)
            .ok_or_else(|| "pipewire format pod invalid".to_string())?];

        stream
            .connect(
                spa::utils::Direction::Input,
                None,
                pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
                &mut params,
            )
            .map_err(|e| format!("pipewire stream connect: {e}"))?;

        let _ = init_tx.try_send(Ok(()));
        mainloop.run();
        Ok(())
    }

    const PULSE_SONAMES: [&str; 2] = ["libpulse.so.0", "libpulse.so"];
    const PULSE_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
    const PULSE_POLL_INTERVAL_US: c_int = 20_000;
    const PULSE_FRAGMENT_MS: u32 = 20;
    const PULSE_MIX_MAX_DELAY: Duration = Duration::from_millis(60);
    const PULSE_STREAM_IDLE: Duration = Duration::from_millis(200);

    const PA_INVALID_INDEX: u32 = u32::MAX;
    const PA_CHANNELS_MAX: usize = 32;
    const PA_SAMPLE_S16LE: c_int = 3;
    const PA_CONTEXT_NOAUTOSPAWN: u32 = 0x1;
    const PA_CONTEXT_READY: c_int = 4;
    const PA_CONTEXT_FAILED: c_int = 5;
    const PA_CONTEXT_TERMINATED: c_int = 6;
    const PA_STREAM_CREATING: c_int = 1;
    const PA_STREAM_READY: c_int = 2;
    const PA_STREAM_FAILED: c_int = 3;
    const PA_STREAM_TERMINATED: c_int = 4;
    const PA_STREAM_DONT_MOVE: u32 = 1 << 9;
    const PA_STREAM_ADJUST_LATENCY: u32 = 1 << 13;
    const PA_SUBSCRIPTION_MASK_SINK_INPUT: u32 = 1 << 2;
    const PA_SUBSCRIPTION_EVENT_FACILITY_MASK: u32 = 0xf;
    const PA_SUBSCRIPTION_EVENT_SINK_INPUT: u32 = 2;
    const PA_SUBSCRIPTION_EVENT_TYPE_MASK: u32 = 0x30;
    const PA_SUBSCRIPTION_EVENT_REMOVE: u32 = 0x20;

    type PaMainloop = c_void;
    type PaMainloopApi = c_void;
    type PaContext = c_void;
    type PaStream = c_void;
    type PaOperation = c_void;
    type PaProplist = c_void;

    #[allow(dead_code)]
    #[repr(C)]
    struct PaSampleSpec {
        format: c_int,
        rate: u32,
        channels: u8,
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct PaChannelMap {
        channels: u8,
        map: [c_int; PA_CHANNELS_MAX],
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct PaCVolume {
        channels: u8,
        values: [u32; PA_CHANNELS_MAX],
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct PaBufferAttr {
        maxlength: u32,
        tlength: u32,
        prebuf: u32,
        minreq: u32,
        fragsize: u32,
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct PaServerInfo {
        user_name: *const c_char,
        host_name: *const c_char,
        server_version: *const c_char,
        server_name: *const c_char,
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct PaSinkInputInfo {
        index: u32,
        name: *const c_char,
        owner_module: u32,
        client: u32,
        sink: u32,
        sample_spec: PaSampleSpec,
        channel_map: PaChannelMap,
        volume: PaCVolume,
        buffer_usec: u64,
        sink_usec: u64,
        resample_method: *const c_char,
        driver: *const c_char,
        mute: c_int,
        proplist: *const PaProplist,
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct PaSinkInfo {
        name: *const c_char,
        index: u32,
        description: *const c_char,
        sample_spec: PaSampleSpec,
        channel_map: PaChannelMap,
        owner_module: u32,
        volume: PaCVolume,
        mute: c_int,
        monitor_source: u32,
    }

    type SubscribeCallback = extern "C" fn(*mut PaContext, u32, u32, *mut c_void);
    type SuccessCallback = extern "C" fn(*mut PaContext, c_int, *mut c_void);
    type ServerInfoCallback = extern "C" fn(*mut PaContext, *const PaServerInfo, *mut c_void);
    type SinkInputInfoCallback =
        extern "C" fn(*mut PaContext, *const PaSinkInputInfo, c_int, *mut c_void);
    type SinkInfoCallback = extern "C" fn(*mut PaContext, *const PaSinkInfo, c_int, *mut c_void);
    type StreamNotifyCallback = extern "C" fn(*mut PaStream, *mut c_void);
    type StreamRequestCallback = extern "C" fn(*mut PaStream, usize, *mut c_void);

    type MainloopNew = unsafe extern "C" fn() -> *mut PaMainloop;
    type MainloopFree = unsafe extern "C" fn(*mut PaMainloop);
    type MainloopGetApi = unsafe extern "C" fn(*mut PaMainloop) -> *mut PaMainloopApi;
    type MainloopPrepare = unsafe extern "C" fn(*mut PaMainloop, c_int) -> c_int;
    type MainloopStep = unsafe extern "C" fn(*mut PaMainloop) -> c_int;
    type ContextNew = unsafe extern "C" fn(*mut PaMainloopApi, *const c_char) -> *mut PaContext;
    type ContextConnect =
        unsafe extern "C" fn(*mut PaContext, *const c_char, u32, *const c_void) -> c_int;
    type ContextQuery = unsafe extern "C" fn(*mut PaContext) -> c_int;
    type ContextRelease = unsafe extern "C" fn(*mut PaContext);
    type ContextSetSubscribeCallback =
        unsafe extern "C" fn(*mut PaContext, Option<SubscribeCallback>, *mut c_void);
    type ContextSubscribe = unsafe extern "C" fn(
        *mut PaContext,
        u32,
        Option<SuccessCallback>,
        *mut c_void,
    ) -> *mut PaOperation;
    type ContextGetServerInfo = unsafe extern "C" fn(
        *mut PaContext,
        Option<ServerInfoCallback>,
        *mut c_void,
    ) -> *mut PaOperation;
    type ContextGetSinkInputInfo = unsafe extern "C" fn(
        *mut PaContext,
        u32,
        Option<SinkInputInfoCallback>,
        *mut c_void,
    ) -> *mut PaOperation;
    type ContextGetSinkInputInfoList = unsafe extern "C" fn(
        *mut PaContext,
        Option<SinkInputInfoCallback>,
        *mut c_void,
    ) -> *mut PaOperation;
    type ContextGetSinkInfoByIndex = unsafe extern "C" fn(
        *mut PaContext,
        u32,
        Option<SinkInfoCallback>,
        *mut c_void,
    ) -> *mut PaOperation;
    type OperationUnref = unsafe extern "C" fn(*mut PaOperation);
    type StreamNew = unsafe extern "C" fn(
        *mut PaContext,
        *const c_char,
        *const PaSampleSpec,
        *const c_void,
    ) -> *mut PaStream;
    type StreamSetMonitorStream = unsafe extern "C" fn(*mut PaStream, u32) -> c_int;
    type StreamConnectRecord =
        unsafe extern "C" fn(*mut PaStream, *const c_char, *const PaBufferAttr, u32) -> c_int;
    type StreamPeek = unsafe extern "C" fn(*mut PaStream, *mut *const c_void, *mut usize) -> c_int;
    type StreamStep = unsafe extern "C" fn(*mut PaStream) -> c_int;
    type StreamUnref = unsafe extern "C" fn(*mut PaStream);
    type StreamSetReadCallback =
        unsafe extern "C" fn(*mut PaStream, Option<StreamRequestCallback>, *mut c_void);
    type StreamSetStateCallback =
        unsafe extern "C" fn(*mut PaStream, Option<StreamNotifyCallback>, *mut c_void);
    type ProplistGets = unsafe extern "C" fn(*const PaProplist, *const c_char) -> *const c_char;
    type Strerror = unsafe extern "C" fn(c_int) -> *const c_char;

    macro_rules! pulse_symbol {
        ($library:expr, $name:literal, $signature:ty) => {
            *unsafe { $library.get::<$signature>(concat!($name, "\0").as_bytes()) }.ok()?
        };
    }

    struct PulseApi {
        mainloop_new: MainloopNew,
        mainloop_free: MainloopFree,
        mainloop_get_api: MainloopGetApi,
        mainloop_prepare: MainloopPrepare,
        mainloop_poll: MainloopStep,
        mainloop_dispatch: MainloopStep,
        context_new: ContextNew,
        context_connect: ContextConnect,
        context_get_state: ContextQuery,
        context_errno: ContextQuery,
        context_disconnect: ContextRelease,
        context_unref: ContextRelease,
        context_set_subscribe_callback: ContextSetSubscribeCallback,
        context_subscribe: ContextSubscribe,
        context_get_server_info: ContextGetServerInfo,
        context_get_sink_input_info: ContextGetSinkInputInfo,
        context_get_sink_input_info_list: ContextGetSinkInputInfoList,
        context_get_sink_info_by_index: ContextGetSinkInfoByIndex,
        operation_unref: OperationUnref,
        stream_new: StreamNew,
        stream_set_monitor_stream: StreamSetMonitorStream,
        stream_connect_record: StreamConnectRecord,
        stream_peek: StreamPeek,
        stream_drop: StreamStep,
        stream_disconnect: StreamStep,
        stream_get_state: StreamStep,
        stream_unref: StreamUnref,
        stream_set_read_callback: StreamSetReadCallback,
        stream_set_state_callback: StreamSetStateCallback,
        proplist_gets: ProplistGets,
        strerror: Strerror,
        _library: Library,
    }

    impl PulseApi {
        fn load() -> Option<Self> {
            let library = PULSE_SONAMES
                .iter()
                .find_map(|soname| unsafe { Library::new(soname) }.ok())?;
            Some(Self {
                mainloop_new: pulse_symbol!(library, "pa_mainloop_new", MainloopNew),
                mainloop_free: pulse_symbol!(library, "pa_mainloop_free", MainloopFree),
                mainloop_get_api: pulse_symbol!(library, "pa_mainloop_get_api", MainloopGetApi),
                mainloop_prepare: pulse_symbol!(library, "pa_mainloop_prepare", MainloopPrepare),
                mainloop_poll: pulse_symbol!(library, "pa_mainloop_poll", MainloopStep),
                mainloop_dispatch: pulse_symbol!(library, "pa_mainloop_dispatch", MainloopStep),
                context_new: pulse_symbol!(library, "pa_context_new", ContextNew),
                context_connect: pulse_symbol!(library, "pa_context_connect", ContextConnect),
                context_get_state: pulse_symbol!(library, "pa_context_get_state", ContextQuery),
                context_errno: pulse_symbol!(library, "pa_context_errno", ContextQuery),
                context_disconnect: pulse_symbol!(library, "pa_context_disconnect", ContextRelease),
                context_unref: pulse_symbol!(library, "pa_context_unref", ContextRelease),
                context_set_subscribe_callback: pulse_symbol!(
                    library,
                    "pa_context_set_subscribe_callback",
                    ContextSetSubscribeCallback
                ),
                context_subscribe: pulse_symbol!(library, "pa_context_subscribe", ContextSubscribe),
                context_get_server_info: pulse_symbol!(
                    library,
                    "pa_context_get_server_info",
                    ContextGetServerInfo
                ),
                context_get_sink_input_info: pulse_symbol!(
                    library,
                    "pa_context_get_sink_input_info",
                    ContextGetSinkInputInfo
                ),
                context_get_sink_input_info_list: pulse_symbol!(
                    library,
                    "pa_context_get_sink_input_info_list",
                    ContextGetSinkInputInfoList
                ),
                context_get_sink_info_by_index: pulse_symbol!(
                    library,
                    "pa_context_get_sink_info_by_index",
                    ContextGetSinkInfoByIndex
                ),
                operation_unref: pulse_symbol!(library, "pa_operation_unref", OperationUnref),
                stream_new: pulse_symbol!(library, "pa_stream_new", StreamNew),
                stream_set_monitor_stream: pulse_symbol!(
                    library,
                    "pa_stream_set_monitor_stream",
                    StreamSetMonitorStream
                ),
                stream_connect_record: pulse_symbol!(
                    library,
                    "pa_stream_connect_record",
                    StreamConnectRecord
                ),
                stream_peek: pulse_symbol!(library, "pa_stream_peek", StreamPeek),
                stream_drop: pulse_symbol!(library, "pa_stream_drop", StreamStep),
                stream_disconnect: pulse_symbol!(library, "pa_stream_disconnect", StreamStep),
                stream_get_state: pulse_symbol!(library, "pa_stream_get_state", StreamStep),
                stream_unref: pulse_symbol!(library, "pa_stream_unref", StreamUnref),
                stream_set_read_callback: pulse_symbol!(
                    library,
                    "pa_stream_set_read_callback",
                    StreamSetReadCallback
                ),
                stream_set_state_callback: pulse_symbol!(
                    library,
                    "pa_stream_set_state_callback",
                    StreamSetStateCallback
                ),
                proplist_gets: pulse_symbol!(library, "pa_proplist_gets", ProplistGets),
                strerror: pulse_symbol!(library, "pa_strerror", Strerror),
                _library: library,
            })
        }
    }

    fn pulse_api() -> Option<&'static PulseApi> {
        static API: OnceLock<Option<PulseApi>> = OnceLock::new();
        API.get_or_init(PulseApi::load).as_ref()
    }

    fn start_pulse_capture() -> Result<Option<ScreenAudioCapture>, String> {
        let (sample_tx, sample_rx) = flume::bounded::<Vec<i16>>(64);
        let (init_tx, init_rx) = flume::bounded::<bool>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();

        let thread = std::thread::Builder::new()
            .name("mezon-screen-audio-pa".into())
            .spawn(move || match PulseCapture::connect() {
                Some(mut capture) => {
                    let _ = init_tx.send(true);
                    capture.run(&sample_tx, &thread_stop);
                }
                None => {
                    let _ = init_tx.send(false);
                }
            })
            .map_err(|e| format!("failed to spawn screen audio thread: {e}"))?;

        if matches!(init_rx.recv(), Ok(true)) {
            Ok(Some(ScreenAudioCapture {
                rx: sample_rx,
                stop: Some(CaptureStop::Pulse(stop)),
                thread: Some(thread),
            }))
        } else {
            let _ = thread.join();
            Ok(None)
        }
    }

    enum PulseEvent {
        ServerName(String),
        SinkInputChanged(u32),
        SinkInputRemoved(u32),
        SinkInput {
            index: u32,
            sink: u32,
            owner_module: u32,
            pid: Option<String>,
            binary: Option<String>,
        },
        Monitor {
            sink: u32,
            source: u32,
        },
        StreamState {
            index: u32,
            serial: u64,
        },
        Samples {
            index: u32,
            samples: Vec<i16>,
        },
    }

    type PulseEvents = RefCell<VecDeque<PulseEvent>>;

    fn push_event(events: *mut c_void, event: PulseEvent) {
        let Some(events) = (unsafe { events.cast::<PulseEvents>().as_ref() }) else {
            return;
        };
        if let Ok(mut queue) = events.try_borrow_mut() {
            queue.push_back(event);
        }
    }

    unsafe fn c_string(value: *const c_char) -> Option<String> {
        if value.is_null() {
            return None;
        }
        Some(
            unsafe { CStr::from_ptr(value) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    fn proplist_value(api: &PulseApi, proplist: *const PaProplist, key: &CStr) -> Option<String> {
        if proplist.is_null() {
            return None;
        }
        unsafe { c_string((api.proplist_gets)(proplist, key.as_ptr())) }
    }

    extern "C" fn on_server_info(
        _context: *mut PaContext,
        info: *const PaServerInfo,
        userdata: *mut c_void,
    ) {
        let info = unsafe { info.as_ref() };
        let name = info.and_then(|info| unsafe { c_string(info.server_name) });
        push_event(userdata, PulseEvent::ServerName(name.unwrap_or_default()));
    }

    extern "C" fn on_subscription(
        _context: *mut PaContext,
        event: u32,
        index: u32,
        userdata: *mut c_void,
    ) {
        if (event & PA_SUBSCRIPTION_EVENT_FACILITY_MASK) != PA_SUBSCRIPTION_EVENT_SINK_INPUT {
            return;
        }
        let event = if (event & PA_SUBSCRIPTION_EVENT_TYPE_MASK) == PA_SUBSCRIPTION_EVENT_REMOVE {
            PulseEvent::SinkInputRemoved(index)
        } else {
            PulseEvent::SinkInputChanged(index)
        };
        push_event(userdata, event);
    }

    extern "C" fn on_sink_input_info(
        _context: *mut PaContext,
        info: *const PaSinkInputInfo,
        eol: c_int,
        userdata: *mut c_void,
    ) {
        if eol != 0 {
            return;
        }
        let (Some(api), Some(info)) = (pulse_api(), unsafe { info.as_ref() }) else {
            return;
        };
        push_event(
            userdata,
            PulseEvent::SinkInput {
                index: info.index,
                sink: info.sink,
                owner_module: info.owner_module,
                pid: proplist_value(api, info.proplist, c"application.process.id"),
                binary: proplist_value(api, info.proplist, c"application.process.binary"),
            },
        );
    }

    extern "C" fn on_sink_info(
        _context: *mut PaContext,
        info: *const PaSinkInfo,
        eol: c_int,
        userdata: *mut c_void,
    ) {
        if eol != 0 {
            return;
        }
        let Some(info) = (unsafe { info.as_ref() }) else {
            return;
        };
        push_event(
            userdata,
            PulseEvent::Monitor {
                sink: info.index,
                source: info.monitor_source,
            },
        );
    }

    struct PulseReader {
        events: *mut c_void,
        index: u32,
        serial: u64,
    }

    extern "C" fn on_stream_state(_stream: *mut PaStream, userdata: *mut c_void) {
        let Some(reader) = (unsafe { userdata.cast::<PulseReader>().as_ref() }) else {
            return;
        };
        push_event(
            reader.events,
            PulseEvent::StreamState {
                index: reader.index,
                serial: reader.serial,
            },
        );
    }

    extern "C" fn on_stream_read(stream: *mut PaStream, _bytes: usize, userdata: *mut c_void) {
        let (Some(api), Some(reader)) = (pulse_api(), unsafe {
            userdata.cast::<PulseReader>().as_ref()
        }) else {
            return;
        };
        let samples = drain_pulse_stream(api, stream);
        if !samples.is_empty() {
            push_event(
                reader.events,
                PulseEvent::Samples {
                    index: reader.index,
                    samples,
                },
            );
        }
    }

    fn drain_pulse_stream(api: &PulseApi, stream: *mut PaStream) -> Vec<i16> {
        let mut samples = Vec::new();
        loop {
            let mut data: *const c_void = std::ptr::null();
            let mut bytes = 0usize;
            if unsafe { (api.stream_peek)(stream, &mut data, &mut bytes) } < 0 || bytes == 0 {
                break;
            }
            if !data.is_null() {
                let chunk = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), bytes) };
                for pair in chunk.chunks_exact(2) {
                    samples.push(i16::from_le_bytes([pair[0], pair[1]]));
                }
            }
            if unsafe { (api.stream_drop)(stream) } < 0 {
                break;
            }
        }
        samples
    }

    #[derive(Default)]
    struct PulseMixer {
        pending: VecDeque<i32>,
        written: HashMap<u32, (usize, Instant)>,
        oldest: Option<Instant>,
    }

    impl PulseMixer {
        fn push(&mut self, index: u32, samples: &[i16], now: Instant) {
            let entry = self.written.entry(index).or_insert((0, now));
            for (offset, &sample) in samples.iter().enumerate() {
                let at = entry.0 + offset;
                if at < self.pending.len() {
                    self.pending[at] += i32::from(sample);
                } else {
                    self.pending.push_back(i32::from(sample));
                }
            }
            entry.0 += samples.len();
            entry.1 = now;
            if self.oldest.is_none() {
                self.oldest = Some(now);
            }
        }

        fn remove(&mut self, index: u32) {
            self.written.remove(&index);
        }

        fn drain(&mut self, now: Instant) -> Option<Vec<i16>> {
            let oldest = self.oldest?;
            let in_step = self
                .written
                .values()
                .filter(|(_, at)| now.duration_since(*at) < PULSE_STREAM_IDLE)
                .map(|(written, _)| *written)
                .min();
            let ready = if now.duration_since(oldest) >= PULSE_MIX_MAX_DELAY {
                self.pending.len()
            } else {
                in_step.unwrap_or(self.pending.len())
            };
            let ready = ready - ready % SCREEN_AUDIO_CHANNELS as usize;
            if ready == 0 {
                return None;
            }
            let mixed: Vec<i16> = self
                .pending
                .drain(..ready)
                .map(|sample| sample.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16)
                .collect();
            for (written, _) in self.written.values_mut() {
                *written = written.saturating_sub(ready);
            }
            self.oldest = (!self.pending.is_empty()).then_some(now);
            Some(mixed)
        }
    }

    struct PulseCaptureStream {
        api: &'static PulseApi,
        stream: *mut PaStream,
        sink: u32,
        serial: u64,
        ready: bool,
        reader: *mut PulseReader,
    }

    impl Drop for PulseCaptureStream {
        fn drop(&mut self) {
            let api = self.api;
            unsafe {
                (api.stream_set_read_callback)(self.stream, None, std::ptr::null_mut());
                (api.stream_set_state_callback)(self.stream, None, std::ptr::null_mut());
                let state = (api.stream_get_state)(self.stream);
                if state == PA_STREAM_CREATING || state == PA_STREAM_READY {
                    (api.stream_disconnect)(self.stream);
                }
                (api.stream_unref)(self.stream);
                drop(Box::from_raw(self.reader));
            }
        }
    }

    struct PulseCapture {
        api: &'static PulseApi,
        mainloop: *mut PaMainloop,
        context: *mut PaContext,
        captures: HashMap<u32, PulseCaptureStream>,
        events: *mut PulseEvents,
        monitors: HashMap<u32, u32>,
        waiting: HashMap<u32, u32>,
        requested_sinks: HashSet<u32>,
        mixer: PulseMixer,
        serial: u64,
        own_pid: String,
        own_binary: Option<String>,
    }

    impl Drop for PulseCapture {
        fn drop(&mut self) {
            self.captures.clear();
            let api = self.api;
            unsafe {
                (api.context_set_subscribe_callback)(self.context, None, std::ptr::null_mut());
                (api.context_disconnect)(self.context);
                (api.context_unref)(self.context);
                (api.mainloop_free)(self.mainloop);
                drop(Box::from_raw(self.events));
            }
        }
    }

    impl PulseCapture {
        fn connect() -> Option<Self> {
            let api = pulse_api()?;
            let mainloop = unsafe { (api.mainloop_new)() };
            if mainloop.is_null() {
                return None;
            }
            let context = unsafe {
                (api.context_new)(
                    (api.mainloop_get_api)(mainloop),
                    c"Mezon screen audio".as_ptr(),
                )
            };
            if context.is_null() {
                unsafe { (api.mainloop_free)(mainloop) };
                return None;
            }
            let mut capture = Self {
                api,
                mainloop,
                context,
                captures: HashMap::new(),
                events: Box::into_raw(Box::new(RefCell::new(VecDeque::new()))),
                monitors: HashMap::new(),
                waiting: HashMap::new(),
                requested_sinks: HashSet::new(),
                mixer: PulseMixer::default(),
                serial: 0,
                own_pid: std::process::id().to_string(),
                own_binary: std::env::current_exe().ok().and_then(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                }),
            };

            let connected = unsafe {
                (api.context_connect)(
                    context,
                    std::ptr::null(),
                    PA_CONTEXT_NOAUTOSPAWN,
                    std::ptr::null(),
                )
            };
            if connected < 0 {
                return None;
            }
            let deadline = Instant::now() + PULSE_CONNECT_TIMEOUT;
            loop {
                match unsafe { (api.context_get_state)(context) } {
                    PA_CONTEXT_READY => break,
                    PA_CONTEXT_FAILED | PA_CONTEXT_TERMINATED => return None,
                    _ => {}
                }
                if Instant::now() >= deadline {
                    return None;
                }
                capture.iterate().ok()?;
            }

            let events = capture.events_ptr();
            capture.forget(unsafe {
                (api.context_get_server_info)(context, Some(on_server_info), events)
            });
            let server_name = loop {
                let event = capture.queue().borrow_mut().pop_front();
                if let Some(PulseEvent::ServerName(name)) = event {
                    break name;
                }
                if Instant::now() >= deadline {
                    return None;
                }
                capture.iterate().ok()?;
            };
            if server_name.contains("PipeWire") {
                return None;
            }
            capture.watch();
            Some(capture)
        }

        fn events_ptr(&self) -> *mut c_void {
            self.events.cast()
        }

        fn queue(&self) -> &PulseEvents {
            unsafe { &*self.events }
        }

        fn forget(&self, operation: *mut PaOperation) {
            if !operation.is_null() {
                unsafe { (self.api.operation_unref)(operation) };
            }
        }

        fn last_error(&self) -> String {
            let api = self.api;
            let message = unsafe { c_string((api.strerror)((api.context_errno)(self.context))) };
            message.unwrap_or_default()
        }

        fn iterate(&mut self) -> Result<(), String> {
            let api = self.api;
            if unsafe { (api.mainloop_prepare)(self.mainloop, PULSE_POLL_INTERVAL_US) } < 0 {
                return Err("PulseAudio mainloop prepare failed".into());
            }
            if unsafe { (api.mainloop_poll)(self.mainloop) } < 0 {
                return Err("PulseAudio mainloop poll failed".into());
            }
            if unsafe { (api.mainloop_dispatch)(self.mainloop) } < 0 {
                return Err("PulseAudio mainloop dispatch failed".into());
            }
            Ok(())
        }

        fn watch(&mut self) {
            let api = self.api;
            let events = self.events_ptr();
            unsafe {
                (api.context_set_subscribe_callback)(self.context, Some(on_subscription), events);
            }
            self.forget(unsafe {
                (api.context_subscribe)(
                    self.context,
                    PA_SUBSCRIPTION_MASK_SINK_INPUT,
                    None,
                    std::ptr::null_mut(),
                )
            });
            self.forget(unsafe {
                (api.context_get_sink_input_info_list)(
                    self.context,
                    Some(on_sink_input_info),
                    events,
                )
            });
        }

        fn run(&mut self, sample_tx: &flume::Sender<Vec<i16>>, stop: &AtomicBool) {
            while !stop.load(Ordering::Relaxed) {
                if let Err(e) = self.iterate() {
                    tracing::warn!("screen audio capture stopped: {e}");
                    return;
                }
                if unsafe { (self.api.context_get_state)(self.context) } != PA_CONTEXT_READY {
                    tracing::warn!("screen audio capture lost its PulseAudio connection");
                    return;
                }
                self.process_events();
                if let Some(mixed) = self.mixer.drain(Instant::now()) {
                    let _ = sample_tx.try_send(mixed);
                }
            }
        }

        fn process_events(&mut self) {
            loop {
                let event = self.queue().borrow_mut().pop_front();
                let Some(event) = event else {
                    return;
                };
                match event {
                    PulseEvent::ServerName(_) => {}
                    PulseEvent::SinkInputChanged(index) => self.request_sink_input(index),
                    PulseEvent::SinkInputRemoved(index) => self.stop_capture(index),
                    PulseEvent::SinkInput {
                        index,
                        sink,
                        owner_module,
                        pid,
                        binary,
                    } => {
                        let relayed_by_module = owner_module != PA_INVALID_INDEX;
                        if relayed_by_module || self.is_own(pid.as_deref(), binary.as_deref()) {
                            self.stop_capture(index);
                        } else {
                            self.follow_sink_input(index, sink);
                        }
                    }
                    PulseEvent::Monitor { sink, source } => {
                        self.monitors.insert(sink, source);
                        let ready: Vec<u32> = self
                            .waiting
                            .iter()
                            .filter(|(_, waiting_sink)| **waiting_sink == sink)
                            .map(|(index, _)| *index)
                            .collect();
                        for index in ready {
                            self.start_capture(index, sink, source);
                        }
                    }
                    PulseEvent::StreamState { index, serial } => {
                        self.stream_state_changed(index, serial)
                    }
                    PulseEvent::Samples { index, samples } => {
                        if self.captures.contains_key(&index) {
                            self.mixer.push(index, &samples, Instant::now());
                        }
                    }
                }
            }
        }

        fn is_own(&self, pid: Option<&str>, binary: Option<&str>) -> bool {
            pid == Some(self.own_pid.as_str())
                || (binary.is_some() && binary == self.own_binary.as_deref())
        }

        fn request_sink_input(&self, index: u32) {
            let api = self.api;
            self.forget(unsafe {
                (api.context_get_sink_input_info)(
                    self.context,
                    index,
                    Some(on_sink_input_info),
                    self.events_ptr(),
                )
            });
        }

        fn follow_sink_input(&mut self, index: u32, sink: u32) {
            if self
                .captures
                .get(&index)
                .is_some_and(|capture| capture.sink == sink)
            {
                return;
            }
            if let Some(source) = self.monitors.get(&sink).copied() {
                self.start_capture(index, sink, source);
                return;
            }
            self.waiting.insert(index, sink);
            if self.requested_sinks.insert(sink) {
                let api = self.api;
                self.forget(unsafe {
                    (api.context_get_sink_info_by_index)(
                        self.context,
                        sink,
                        Some(on_sink_info),
                        self.events_ptr(),
                    )
                });
            }
        }

        fn start_capture(&mut self, index: u32, sink: u32, source: u32) {
            self.stop_capture(index);
            let api = self.api;
            let spec = PaSampleSpec {
                format: PA_SAMPLE_S16LE,
                rate: SCREEN_AUDIO_SAMPLE_RATE,
                channels: SCREEN_AUDIO_CHANNELS as u8,
            };
            let stream = unsafe {
                (api.stream_new)(
                    self.context,
                    c"Mezon screen audio".as_ptr(),
                    &spec,
                    std::ptr::null(),
                )
            };
            if stream.is_null() {
                tracing::warn!(id = index, "screen audio PulseAudio stream not created");
                return;
            }
            self.serial += 1;
            let reader = Box::into_raw(Box::new(PulseReader {
                events: self.events_ptr(),
                index,
                serial: self.serial,
            }));
            let userdata: *mut c_void = reader.cast();
            let capture = PulseCaptureStream {
                api,
                stream,
                sink,
                serial: self.serial,
                ready: false,
                reader,
            };
            let attr = PaBufferAttr {
                maxlength: u32::MAX,
                tlength: u32::MAX,
                prebuf: u32::MAX,
                minreq: u32::MAX,
                fragsize: SCREEN_AUDIO_SAMPLE_RATE / 1000
                    * PULSE_FRAGMENT_MS
                    * SCREEN_AUDIO_CHANNELS
                    * 2,
            };
            let Ok(device) = CString::new(source.to_string()) else {
                return;
            };
            let connected = unsafe {
                (api.stream_set_state_callback)(stream, Some(on_stream_state), userdata);
                (api.stream_set_read_callback)(stream, Some(on_stream_read), userdata);
                (api.stream_set_monitor_stream)(stream, index) == 0
                    && (api.stream_connect_record)(
                        stream,
                        device.as_ptr(),
                        &attr,
                        PA_STREAM_ADJUST_LATENCY | PA_STREAM_DONT_MOVE,
                    ) == 0
            };
            if !connected {
                tracing::warn!(
                    id = index,
                    "screen audio PulseAudio capture failed: {}",
                    self.last_error()
                );
                return;
            }
            self.captures.insert(index, capture);
        }

        fn stop_capture(&mut self, index: u32) {
            self.waiting.remove(&index);
            self.mixer.remove(index);
            self.captures.remove(&index);
        }

        fn stream_state_changed(&mut self, index: u32, serial: u64) {
            let api = self.api;
            let Some(capture) = self
                .captures
                .get_mut(&index)
                .filter(|capture| capture.serial == serial)
            else {
                return;
            };
            match unsafe { (api.stream_get_state)(capture.stream) } {
                PA_STREAM_READY => capture.ready = true,
                PA_STREAM_FAILED | PA_STREAM_TERMINATED => {
                    let was_ready = capture.ready;
                    self.stop_capture(index);
                    if was_ready {
                        self.request_sink_input(index);
                    } else {
                        tracing::warn!(id = index, "screen audio PulseAudio stream never started");
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod wasapi {
    use std::mem::ManuallyDrop;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::{
        AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
        AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
        AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync,
        IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
        IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
        PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        WAVE_FORMAT_PCM, WAVEFORMATEX,
    };
    use windows::Win32::System::Com::StructuredStorage::{
        PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
    };
    use windows::Win32::System::Com::{
        BLOB, COINIT_DISABLE_OLE1DDE, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
    use windows::Win32::System::Variant::VT_BLOB;
    use windows::core::{HRESULT, IUnknown, Interface, PCWSTR, Ref, implement};

    use super::{SCREEN_AUDIO_CHANNELS, SCREEN_AUDIO_SAMPLE_RATE};

    const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(5);
    const STARTUP_TIMEOUT: Duration = Duration::from_secs(8);
    const BUFFER_DURATION_100NS: i64 = 2_000_000;
    const WAIT_SLICE_MS: u32 = 100;
    const BYTES_PER_SAMPLE: u32 = 2;
    const MAX_PACKET_FRAMES: u32 = SCREEN_AUDIO_SAMPLE_RATE;

    #[derive(Clone, Copy)]
    struct EventHandle(isize);

    impl EventHandle {
        fn raw(self) -> HANDLE {
            HANDLE(self.0 as *mut core::ffi::c_void)
        }
    }

    pub struct ScreenAudioCapture {
        pub rx: flume::Receiver<Vec<i16>>,
        stop: Arc<AtomicBool>,
        wake: EventHandle,
        thread: Option<JoinHandle<()>>,
    }

    impl Drop for ScreenAudioCapture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            unsafe {
                let _ = SetEvent(self.wake.raw());
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            unsafe {
                let _ = CloseHandle(self.wake.raw());
            }
        }
    }

    #[implement(IActivateAudioInterfaceCompletionHandler)]
    struct ActivationDone {
        tx: flume::Sender<()>,
    }

    #[allow(non_snake_case)]
    impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationDone_Impl {
        fn ActivateCompleted(
            &self,
            _operation: Ref<'_, IActivateAudioInterfaceAsyncOperation>,
        ) -> windows::core::Result<()> {
            let _ = self.tx.try_send(());
            Ok(())
        }
    }

    pub fn start_screen_audio() -> Result<ScreenAudioCapture, String> {
        let wake = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map_err(|e| format!("screen audio event: {e}"))?;
        let wake = EventHandle(wake.0 as isize);
        let (sample_tx, sample_rx) = flume::bounded::<Vec<i16>>(64);
        let (init_tx, init_rx) = flume::bounded::<Result<(), String>>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("mezon-screen-audio".into())
            .spawn(move || {
                let com =
                    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED | COINIT_DISABLE_OLE1DDE) };
                if com.is_err() {
                    let _ = init_tx.try_send(Err(format!("screen audio COM init failed: {com}")));
                    return;
                }
                tracing::info!("screen audio step: com initialized");
                capture_loop(wake, &thread_stop, &sample_tx, &init_tx);
                unsafe { CoUninitialize() };
                tracing::info!("screen audio step: thread finished");
            })
            .map_err(|e| format!("screen audio thread: {e}"))?;

        match init_rx.recv_timeout(STARTUP_TIMEOUT) {
            Ok(Ok(())) => Ok(ScreenAudioCapture {
                rx: sample_rx,
                stop,
                wake,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                unsafe {
                    let _ = CloseHandle(wake.raw());
                }
                Err(e)
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                unsafe {
                    let _ = SetEvent(wake.raw());
                }
                let _ = thread.join();
                unsafe {
                    let _ = CloseHandle(wake.raw());
                }
                Err("screen audio activation timed out".into())
            }
        }
    }

    fn capture_loop(
        wake: EventHandle,
        stop: &AtomicBool,
        tx: &flume::Sender<Vec<i16>>,
        init_tx: &flume::Sender<Result<(), String>>,
    ) {
        let (client, capture) = match open_process_loopback(wake.raw()) {
            Ok(pair) => pair,
            Err(e) => {
                let _ = init_tx.try_send(Err(e));
                return;
            }
        };
        tracing::info!("screen audio step: loopback opened");
        if let Err(e) = unsafe { client.Start() } {
            let _ = init_tx.try_send(Err(format!("screen audio start failed: {e}")));
            return;
        }
        tracing::info!("screen audio step: client started");
        let _ = init_tx.try_send(Ok(()));

        let channels = SCREEN_AUDIO_CHANNELS as usize;
        let mut logged_first = false;
        while !stop.load(Ordering::Relaxed) {
            if unsafe { WaitForSingleObject(wake.raw(), WAIT_SLICE_MS) } != WAIT_OBJECT_0 {
                continue;
            }
            drain_packets(&capture, channels, &mut logged_first, tx);
        }
        tracing::info!("screen audio step: capture loop left");
        unsafe {
            let _ = client.Stop();
        }
    }

    fn drain_packets(
        capture: &IAudioCaptureClient,
        channels: usize,
        logged_first: &mut bool,
        tx: &flume::Sender<Vec<i16>>,
    ) {
        loop {
            let pending = unsafe { capture.GetNextPacketSize() }.unwrap_or(0);
            if pending == 0 {
                return;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            if unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }.is_err()
            {
                return;
            }
            if !*logged_first {
                *logged_first = true;
                tracing::info!(pending, frames, flags, "screen audio step: first packet");
            }
            let silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
            let oversized = frames > MAX_PACKET_FRAMES;
            if oversized {
                tracing::error!(
                    frames,
                    limit = MAX_PACKET_FRAMES,
                    "screen audio packet exceeds the packet limit; dropping it"
                );
            }
            if frames > 0 && !silent && !oversized && !data.is_null() {
                let samples = unsafe {
                    std::slice::from_raw_parts(data.cast::<i16>(), frames as usize * channels)
                };
                let _ = tx.try_send(samples.to_vec());
            }
            if unsafe { capture.ReleaseBuffer(frames) }.is_err() {
                return;
            }
        }
    }

    fn open_process_loopback(wake: HANDLE) -> Result<(IAudioClient, IAudioCaptureClient), String> {
        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: std::process::id(),
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        let activation = ManuallyDrop::new(PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 {
                        blob: BLOB {
                            cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                            pBlobData: std::ptr::from_mut(&mut params).cast::<u8>(),
                        },
                    },
                }),
            },
        });

        let (done_tx, done_rx) = flume::bounded::<()>(1);
        let handler: IActivateAudioInterfaceCompletionHandler =
            ActivationDone { tx: done_tx }.into();
        tracing::info!(pid = std::process::id(), "screen audio step: activating");
        let operation = unsafe {
            ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(std::ptr::from_ref(&*activation)),
                &handler,
            )
        }
        .map_err(|e| format!("process loopback activation failed: {e}"))?;
        tracing::info!("screen audio step: activation requested");
        done_rx
            .recv_timeout(ACTIVATION_TIMEOUT)
            .map_err(|_| "process loopback activation timed out".to_string())?;
        tracing::info!("screen audio step: activation completed");

        let mut result = HRESULT(0);
        let mut activated: Option<IUnknown> = None;
        unsafe { operation.GetActivateResult(&mut result, &mut activated) }
            .map_err(|e| format!("process loopback result unavailable: {e}"))?;
        result
            .ok()
            .map_err(|e| format!("process loopback unavailable: {e}"))?;
        let client: IAudioClient = activated
            .ok_or_else(|| "process loopback returned no audio client".to_string())?
            .cast()
            .map_err(|e| format!("process loopback client: {e}"))?;
        tracing::info!("screen audio step: audio client obtained");

        let block_align = SCREEN_AUDIO_CHANNELS * BYTES_PER_SAMPLE;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: SCREEN_AUDIO_CHANNELS as u16,
            nSamplesPerSec: SCREEN_AUDIO_SAMPLE_RATE,
            nAvgBytesPerSec: SCREEN_AUDIO_SAMPLE_RATE * block_align,
            nBlockAlign: block_align as u16,
            wBitsPerSample: (BYTES_PER_SAMPLE * 8) as u16,
            cbSize: 0,
        };
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                BUFFER_DURATION_100NS,
                0,
                &format,
                None,
            )
        }
        .map_err(|e| format!("process loopback init failed: {e}"))?;
        tracing::info!(
            rate = SCREEN_AUDIO_SAMPLE_RATE,
            channels = SCREEN_AUDIO_CHANNELS,
            "screen audio step: client initialized"
        );
        unsafe { client.SetEventHandle(wake) }
            .map_err(|e| format!("process loopback event: {e}"))?;
        let capture: IAudioCaptureClient = unsafe { client.GetService() }
            .map_err(|e| format!("process loopback capture client: {e}"))?;
        tracing::info!("screen audio step: capture client obtained");
        Ok((client, capture))
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use objc::runtime::YES;
    use objc::{msg_send, sel, sel_impl};
    use screencapturekit_sys::audio_buffer::CopiedAudioBuffer;
    use screencapturekit_sys::cm_sample_buffer_ref::CMSampleBufferRef;
    use screencapturekit_sys::content_filter::{UnsafeContentFilter, UnsafeInitParams};
    use screencapturekit_sys::os_types::base::CMTime;
    use screencapturekit_sys::os_types::rc::Id;
    use screencapturekit_sys::shareable_content::UnsafeSCShareableContent;
    use screencapturekit_sys::stream::UnsafeSCStream;
    use screencapturekit_sys::stream_configuration::{
        UnsafeStreamConfiguration, UnsafeStreamConfigurationRef,
    };
    use screencapturekit_sys::stream_error_handler::UnsafeSCStreamError;
    use screencapturekit_sys::stream_output_handler::UnsafeSCStreamOutput;

    use super::{SCREEN_AUDIO_CHANNELS, SCREEN_AUDIO_SAMPLE_RATE};

    const AUDIO_OUTPUT_TYPE: u8 = 1;

    pub struct ScreenAudioCapture {
        stream: Id<UnsafeSCStream>,
        pub rx: flume::Receiver<Vec<i16>>,
    }

    impl Drop for ScreenAudioCapture {
        fn drop(&mut self) {
            let _ = self.stream.stop_capture();
        }
    }

    struct AudioOutput {
        tx: flume::Sender<Vec<i16>>,
    }

    impl UnsafeSCStreamOutput for AudioOutput {
        fn did_output_sample_buffer(&self, sample: Id<CMSampleBufferRef>, of_type: u8) {
            if of_type != AUDIO_OUTPUT_TYPE {
                return;
            }
            let buffers = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sample.get_av_audio_buffer_list()
            })) {
                Ok(buffers) => buffers,
                Err(_) => {
                    return;
                }
            };
            let interleaved = interleave_stereo_i16(&buffers);
            if !interleaved.is_empty() {
                let _ = self.tx.try_send(interleaved);
            }
        }
    }

    struct AudioErrorHandler;

    impl UnsafeSCStreamError for AudioErrorHandler {
        fn handle_error(&self) {
            tracing::warn!("screen audio capture stream error");
        }
    }

    pub fn start_screen_audio() -> Result<ScreenAudioCapture, String> {
        let content = UnsafeSCShareableContent::get()
            .map_err(|e| format!("shareable content unavailable: {e}"))?;
        let display = content
            .displays()
            .into_iter()
            .next()
            .ok_or_else(|| "no display available for system audio".to_string())?;
        let filter = UnsafeContentFilter::init(UnsafeInitParams::Display(display));
        let config: Id<UnsafeStreamConfigurationRef> = UnsafeStreamConfiguration {
            width: 2,
            height: 2,
            captures_audio: 1,
            sample_rate: SCREEN_AUDIO_SAMPLE_RATE,
            channel_count: SCREEN_AUDIO_CHANNELS,
            excludes_current_process_audio: 1,
            minimum_frame_interval: CMTime {
                value: 1,
                timescale: 1,
                epoch: 0,
                flags: 1,
            },
            ..Default::default()
        }
        .into();
        unsafe {
            let _: () = msg_send![config, setExcludesCurrentProcessAudio: YES];
            let _: () = msg_send![config, setSampleRate: i64::from(SCREEN_AUDIO_SAMPLE_RATE)];
            let _: () = msg_send![config, setChannelCount: i64::from(SCREEN_AUDIO_CHANNELS)];
        }
        let (tx, rx) = flume::bounded::<Vec<i16>>(64);
        let stream = UnsafeSCStream::init(filter, config, AudioErrorHandler);
        stream.add_stream_output(AudioOutput { tx }, AUDIO_OUTPUT_TYPE);
        stream
            .start_capture()
            .map_err(|e| format!("screen audio start failed: {e}"))?;
        Ok(ScreenAudioCapture { stream, rx })
    }

    fn f32_samples(bytes: &[u8]) -> impl Iterator<Item = f32> + '_ {
        bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn to_i16(sample: f32) -> i16 {
        (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
    }

    fn interleave_stereo_i16(buffers: &[CopiedAudioBuffer]) -> Vec<i16> {
        match buffers {
            [] => Vec::new(),
            [interleaved] => f32_samples(&interleaved.data).map(to_i16).collect(),
            [left, right, ..] => {
                let mut out = Vec::with_capacity(left.data.len() / 2);
                for (l, r) in f32_samples(&left.data).zip(f32_samples(&right.data)) {
                    out.push(to_i16(l));
                    out.push(to_i16(r));
                }
                out
            }
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod probe {
    //! Run with `cargo test -p mezon-voice --lib probe -- --ignored --nocapture`
    //! while something outside Mezon is playing sound.
    use std::time::{Duration, Instant};

    #[test]
    #[ignore]
    fn shared_system_audio_reaches_the_recorded_file() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let path = std::env::temp_dir().join("mezon-screen-audio-probe.mp4");
        let _ = std::fs::remove_file(&path);
        let recorder = mezon_record::Recorder::start(mezon_record::RecorderConfig {
            path: path.clone(),
            video: None,
        })
        .expect("recorder");

        let taps = crate::record::RecordTaps::default();
        taps.set(Some(recorder.audio_tap()));

        let capture = super::start_screen_audio().expect("screen audio");
        let stop = Arc::new(AtomicBool::new(false));

        let pump_taps = taps.clone();
        let pump_rx = capture.rx.clone();
        let pump_stop = stop.clone();
        let pump = std::thread::spawn(move || {
            while !pump_stop.load(Ordering::Relaxed) {
                let Ok(samples) = pump_rx.recv_timeout(Duration::from_millis(100)) else {
                    continue;
                };
                if samples.len() as u32 / super::SCREEN_AUDIO_CHANNELS == 0 {
                    continue;
                }
                pump_taps.push(
                    mezon_record::AudioSource::Screen,
                    &samples,
                    super::SCREEN_AUDIO_SAMPLE_RATE,
                    super::SCREEN_AUDIO_CHANNELS,
                );
            }
        });

        // The playback device tees silence into the recorder the whole call,
        // and that is what paces the mixer, so a faithful run needs it.
        let silence = vec![0i16; 480 * 2];
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            taps.push(mezon_record::AudioSource::Remote, &silence, 48_000, 2);
            std::thread::sleep(Duration::from_millis(10));
        }

        stop.store(true, Ordering::Relaxed);
        pump.join().expect("pump");
        taps.set(None);
        let written = recorder.finish().expect("finish");

        let wav = written.with_extension("probe.wav");
        let status = std::process::Command::new("afconvert")
            .args(["-f", "WAVE", "-d", "LEI16"])
            .arg(&written)
            .arg(&wav)
            .status()
            .expect("afconvert");
        assert!(status.success(), "afconvert could not read the recording");
        let bytes = std::fs::read(&wav).expect("wav");
        let peak = bytes[44..]
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]).unsigned_abs() as i32)
            .max()
            .unwrap_or(0);
        println!("recorded {} bytes, audio peak={peak}", bytes.len());
        assert!(peak > 500, "the shared system audio never reached the file");
    }

    /// The real app already holds an SCStream for the shared video when the
    /// audio one is opened, so a second stream has to be allowed.
    #[test]
    #[ignore]
    fn a_second_capture_stream_still_delivers() {
        let first = match super::start_screen_audio() {
            Ok(capture) => capture,
            Err(error) => panic!("first start_screen_audio failed: {error}"),
        };
        let second = match super::start_screen_audio() {
            Ok(capture) => capture,
            Err(error) => panic!("second start_screen_audio failed: {error}"),
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        let (mut a, mut b) = (0u64, 0u64);
        while Instant::now() < deadline {
            if first.rx.recv_timeout(Duration::from_millis(50)).is_ok() {
                a += 1;
            }
            if second.rx.recv_timeout(Duration::from_millis(50)).is_ok() {
                b += 1;
            }
        }
        println!("first={a} second={b}");
        assert!(a > 0 && b > 0, "one of the two streams went silent");
    }

    #[test]
    #[ignore]
    fn system_audio_capture_delivers_samples() {
        let capture = match super::start_screen_audio() {
            Ok(capture) => capture,
            Err(error) => panic!("start_screen_audio failed: {error}"),
        };
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut buffers = 0u64;
        let mut samples = 0u64;
        let mut peak = 0i32;
        while Instant::now() < deadline {
            if let Ok(chunk) = capture.rx.recv_timeout(Duration::from_millis(250)) {
                buffers += 1;
                samples += chunk.len() as u64;
                for value in chunk {
                    peak = peak.max(value.unsigned_abs() as i32);
                }
            }
        }
        println!("buffers={buffers} samples={samples} peak={peak}");
    }
}
