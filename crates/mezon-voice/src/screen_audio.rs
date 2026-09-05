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
    use std::collections::{HashMap, HashSet};
    use std::rc::Rc;
    use std::thread::JoinHandle;

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

    struct Graph {
        own_pid: String,
        own_binary: Option<String>,
        core: pw::core::Core,
        own_node: Option<u32>,
        own_ports: HashMap<String, u32>,
        app_nodes: HashSet<u32>,
        ports: HashMap<u32, PortInfo>,
        links: HashMap<u32, Vec<pw::link::Link>>,
    }

    impl Graph {
        fn new(core: pw::core::Core) -> Self {
            Self {
                own_pid: std::process::id().to_string(),
                own_binary: std::env::current_exe()
                    .ok()
                    .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned())),
                core,
                own_node: None,
                own_ports: HashMap::new(),
                app_nodes: HashSet::new(),
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

        fn global_added(&mut self, global: &GlobalObject<&DictRef>) {
            let Some(props) = global.props else { return };
            match global.type_ {
                ObjectType::Node => self.node_added(global.id, props),
                ObjectType::Port => self.port_added(global.id, props),
                _ => {}
            }
        }

        fn node_added(&mut self, id: u32, props: &DictRef) {
            if props.get(*pw::keys::MEDIA_CLASS) != Some(OUTPUT_STREAM_CLASS) {
                return;
            }
            let pid = props.get(*pw::keys::APP_PROCESS_ID);
            let binary = props.get(*pw::keys::APP_PROCESS_BINARY);
            let own = pid == Some(self.own_pid.as_str())
                || (binary.is_some() && binary == self.own_binary.as_deref());
            tracing::debug!(
                id,
                app = props.get(*pw::keys::APP_NAME).unwrap_or("?"),
                pid = pid.unwrap_or("?"),
                binary = binary.unwrap_or("?"),
                own,
                "screen audio output stream seen"
            );
            if own {
                return;
            }
            self.app_nodes.insert(id);
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
                    port.output && self.app_nodes.contains(&port.node) && !self.links.contains_key(id)
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
                            tracing::debug!(port, target, "screen audio app port linked");
                            created.push(link);
                        }
                        Err(e) => tracing::warn!("screen audio link {port}->{target} failed: {e}"),
                    }
                }
                self.links.insert(port, created);
            }
        }
    }

    pub struct ScreenAudioCapture {
        pub rx: flume::Receiver<Vec<i16>>,
        terminate: Option<pw::channel::Sender<Terminate>>,
        thread: Option<JoinHandle<()>>,
    }

    impl Drop for ScreenAudioCapture {
        fn drop(&mut self) {
            if let Some(terminate) = self.terminate.take() {
                let _ = terminate.send(Terminate);
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    pub fn start_screen_audio() -> Result<ScreenAudioCapture, String> {
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
                terminate: Some(terminate_tx),
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

        let registry = core
            .get_registry()
            .map_err(|e| format!("pipewire registry: {e}"))?;
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
                move |global| graph.borrow_mut().global_added(global)
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
        ActivateAudioInterfaceAsync, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
        AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
        AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
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
                let com = unsafe {
                    CoInitializeEx(None, COINIT_MULTITHREADED | COINIT_DISABLE_OLE1DDE)
                };
                if com.is_err() {
                    let _ = init_tx.try_send(Err(format!("screen audio COM init failed: {com}")));
                    return;
                }
                capture_loop(wake, &thread_stop, &sample_tx, &init_tx);
                unsafe { CoUninitialize() };
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
        if let Err(e) = unsafe { client.Start() } {
            let _ = init_tx.try_send(Err(format!("screen audio start failed: {e}")));
            return;
        }
        let _ = init_tx.try_send(Ok(()));

        let channels = SCREEN_AUDIO_CHANNELS as usize;
        while !stop.load(Ordering::Relaxed) {
            if unsafe { WaitForSingleObject(wake.raw(), WAIT_SLICE_MS) } != WAIT_OBJECT_0 {
                continue;
            }
            drain_packets(&capture, channels, tx);
        }
        unsafe {
            let _ = client.Stop();
        }
    }

    fn drain_packets(capture: &IAudioCaptureClient, channels: usize, tx: &flume::Sender<Vec<i16>>) {
        loop {
            let pending = unsafe { capture.GetNextPacketSize() }.unwrap_or(0);
            if pending == 0 {
                return;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            if unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }.is_err() {
                return;
            }
            let silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
            if frames > 0 && !silent && !data.is_null() {
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
        let activation = PROPVARIANT {
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
        };

        let (done_tx, done_rx) = flume::bounded::<()>(1);
        let handler: IActivateAudioInterfaceCompletionHandler =
            ActivationDone { tx: done_tx }.into();
        let operation = unsafe {
            ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(std::ptr::from_ref(&activation)),
                &handler,
            )
        }
        .map_err(|e| format!("process loopback activation failed: {e}"))?;
        done_rx
            .recv_timeout(ACTIVATION_TIMEOUT)
            .map_err(|_| "process loopback activation timed out".to_string())?;

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
        unsafe { client.SetEventHandle(wake) }
            .map_err(|e| format!("process loopback event: {e}"))?;
        let capture: IAudioCaptureClient = unsafe { client.GetService() }
            .map_err(|e| format!("process loopback capture client: {e}"))?;
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

    /// The whole production chain minus the SFU: the real ScreenCaptureKit
    /// stream, the same pump loop `start_screen_audio` runs, the real
    /// `RecordTaps` the recorder is wired through, and a real encoded file.
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
