use std::{
    mem::size_of,
    sync::{
        atomic::{AtomicBool, AtomicU8},
        mpsc::{sync_channel, RecvError, SendError, Sender, SyncSender},
        Arc,
    },
    thread::JoinHandle,
    time::Duration,
};

use anyhow::{anyhow, Context as _, Result};
use pipewire as pw;
use pw::{
    context::Context,
    main_loop::MainLoop,
    properties::properties,
    spa::{
        self,
        param::{
            format::{FormatProperties, MediaSubtype, MediaType},
            video::VideoFormat,
            ParamType,
        },
        pod::{Pod, Property},
        sys::{
            spa_buffer, spa_meta_header, spa_meta_region, SPA_META_Header, SPA_META_VideoCrop,
            SPA_PARAM_META_size, SPA_PARAM_META_type,
        },
        utils::{Direction, SpaTypes},
    },
    stream::{StreamRef, StreamState},
};

use crate::{
    capturer::Options,
    frame::{BGRxFrame, Frame, RGBFrame, RGBxFrame, XBGRFrame},
    Target,
};

use self::portal::ScreenCastPortal;

use super::LinuxCapturerImpl;

mod portal;

static LOGGED_FIRST_FRAME: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
struct ListenerUserData {
    pub tx: Sender<Result<Frame>>,
    pub format: spa::param::video::VideoInfoRaw,
    pub stream_error: Arc<AtomicBool>,
}

fn param_changed_callback(
    stream: &StreamRef,
    user_data: &mut ListenerUserData,
    id: u32,
    param: Option<&Pod>,
) {
    let Some(param) = param else {
        return;
    };
    if id != pw::spa::param::ParamType::Format.as_raw() {
        return;
    }
    let (media_type, media_subtype) = match pw::spa::param::format_utils::parse_format(param) {
        Ok(v) => v,
        Err(_) => return,
    };

    if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
        return;
    }

    user_data
        .format
        .parse(param)
        // TODO: Tell library user of the error
        .expect("Failed to parse format parameter");

    match serialize_meta_params() {
        Ok((header_values, crop_values)) => {
            let pods = (
                pw::spa::pod::Pod::from_bytes(&header_values),
                pw::spa::pod::Pod::from_bytes(&crop_values),
            );
            if let (Some(header_pod), Some(crop_pod)) = pods {
                let mut params = [header_pod, crop_pod];
                if let Err(e) = stream.update_params(&mut params) {
                    log::error!("Failed to update stream meta params: {e}");
                }
            }
        }
        Err(e) => log::error!("Failed to serialize stream meta params: {e}"),
    }
}

fn serialize_meta_params() -> Result<(Vec<u8>, Vec<u8>)> {
    let metas_obj = pw::spa::pod::object!(
        SpaTypes::ObjectParamMeta,
        ParamType::Meta,
        Property::new(
            SPA_PARAM_META_type,
            pw::spa::pod::Value::Id(pw::spa::utils::Id(SPA_META_Header))
        ),
        Property::new(
            SPA_PARAM_META_size,
            pw::spa::pod::Value::Int(size_of::<pw::spa::sys::spa_meta_header>() as i32)
        ),
    );
    let crop_meta_obj = pw::spa::pod::object!(
        SpaTypes::ObjectParamMeta,
        ParamType::Meta,
        Property::new(
            SPA_PARAM_META_type,
            pw::spa::pod::Value::Id(pw::spa::utils::Id(SPA_META_VideoCrop))
        ),
        Property::new(
            SPA_PARAM_META_size,
            pw::spa::pod::Value::Int(size_of::<spa_meta_region>() as i32)
        ),
    );
    let header_values = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(metas_obj),
    )?
    .0
    .into_inner();
    let crop_values = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(crop_meta_obj),
    )?
    .0
    .into_inner();
    Ok((header_values, crop_values))
}

fn state_changed_callback(
    _stream: &StreamRef,
    user_data: &mut ListenerUserData,
    _old: StreamState,
    new: StreamState,
) {
    match new {
        StreamState::Error(e) => {
            log::debug!("pipewire: State changed to error({e})");
            user_data
                .stream_error
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        _ => {}
    }
}

unsafe fn get_timestamp(buffer: *mut spa_buffer) -> i64 {
    let n_metas = (*buffer).n_metas;
    if n_metas > 0 {
        let mut meta_ptr = (*buffer).metas;
        let metas_end = (*buffer).metas.wrapping_add(n_metas as usize);
        while meta_ptr != metas_end {
            if (*meta_ptr).type_ == SPA_META_Header {
                let meta_header: &mut spa_meta_header =
                    &mut *((*meta_ptr).data as *mut spa_meta_header);
                return meta_header.pts;
            }
            meta_ptr = meta_ptr.wrapping_add(1);
        }
        0
    } else {
        0
    }
}

unsafe fn get_video_crop(buffer: *mut spa_buffer) -> Option<(u32, u32, u32, u32)> {
    let n_metas = (*buffer).n_metas;
    let mut meta_ptr = (*buffer).metas;
    if meta_ptr.is_null() {
        return None;
    }
    let metas_end = meta_ptr.wrapping_add(n_metas as usize);
    while meta_ptr != metas_end {
        if (*meta_ptr).type_ == SPA_META_VideoCrop
            && (*meta_ptr).size as usize >= size_of::<spa_meta_region>()
        {
            let region_ptr = (*meta_ptr).data as *const spa_meta_region;
            if region_ptr.is_null() {
                return None;
            }
            let region = (*region_ptr).region;
            if region.position.x >= 0
                && region.position.y >= 0
                && region.size.width > 0
                && region.size.height > 0
            {
                return Some((
                    region.position.x as u32,
                    region.position.y as u32,
                    region.size.width,
                    region.size.height,
                ));
            }
            return None;
        }
        meta_ptr = meta_ptr.wrapping_add(1);
    }
    None
}

fn process_callback(stream: &StreamRef, user_data: &mut ListenerUserData) {
    let buffer = unsafe { stream.dequeue_raw_buffer() };
    let frame_result = match process_callback_impl(buffer, user_data) {
        Ok(None) => None,
        Ok(Some(frame)) => Some(Ok(frame)),
        Err(err) => Some(Err(err)),
    };
    if let Some(frame_result) = frame_result {
        match user_data.tx.send(frame_result) {
            Ok(()) => {}
            Err(SendError(_)) => {
                log::debug!("Frame receiver was dropped.")
            }
        }
    }
    unsafe { stream.queue_raw_buffer(buffer) };
}

fn process_callback_impl(
    buffer: *mut pipewire::sys::pw_buffer,
    user_data: &mut ListenerUserData,
) -> Result<Option<Frame>> {
    if buffer.is_null() {
        return Err(anyhow!("Wayland screen capture out of buffers."));
    }
    let buffer = unsafe { (*buffer).buffer };
    if buffer.is_null() {
        // TODO: This matches the behavior of the original code by not having an error here.
        log::error!("Buffer pointer unexpectedly null in Wayland screen capture.");
        return Ok(None);
    }

    let timestamp = unsafe { get_timestamp(buffer) };

    let n_datas = unsafe { (*buffer).n_datas };
    if n_datas < 1 {
        return Ok(None);
    }
    let frame_size = user_data.format.size();
    let full_w = frame_size.width;
    let full_h = frame_size.height;
    if full_w == 0 || full_h == 0 {
        return Ok(None);
    }

    let format = user_data.format.format();
    let bpp: usize = if format == VideoFormat::RGB { 3 } else { 4 };

    let frame_data: Vec<u8>;
    let out_w;
    let out_h;
    unsafe {
        let datas = (*buffer).datas;
        let data_ptr = (*datas).data as *const u8;
        if data_ptr.is_null() {
            return Ok(None);
        }
        let src = std::slice::from_raw_parts(data_ptr, (*datas).maxsize as usize);
        let chunk = (*datas).chunk;
        let (offset, stride) = if chunk.is_null() {
            (0usize, 0usize)
        } else {
            ((*chunk).offset as usize, (*chunk).stride.max(0) as usize)
        };
        let stride = if stride > 0 {
            stride
        } else {
            full_w as usize * bpp
        };

        let crop = get_video_crop(buffer).filter(|&(x, y, w, h)| {
            x.checked_add(w).is_some_and(|right| right <= full_w)
                && y.checked_add(h).is_some_and(|bottom| bottom <= full_h)
        });
        if !LOGGED_FIRST_FRAME.swap(true, std::sync::atomic::Ordering::Relaxed) {
            log::info!(
                "pipewire frame: stream {}x{}, stride {}, crop {:?}",
                full_w,
                full_h,
                stride,
                crop
            );
        }
        let (crop_x, crop_y, w, h) = crop.unwrap_or((0, 0, full_w, full_h));
        out_w = w;
        out_h = h;

        let mut data = Vec::with_capacity(w as usize * h as usize * bpp);
        for row in 0..h as usize {
            let start = offset + (crop_y as usize + row) * stride + crop_x as usize * bpp;
            let end = start + w as usize * bpp;
            if end > src.len() {
                return Ok(None);
            }
            data.extend_from_slice(&src[start..end]);
        }
        frame_data = data;
    }

    match format {
        VideoFormat::RGBx => Ok(Some(Frame::RGBx(RGBxFrame {
            display_time: timestamp as u64,
            width: out_w as i32,
            height: out_h as i32,
            data: frame_data,
        }))),
        VideoFormat::RGB => Ok(Some(Frame::RGB(RGBFrame {
            display_time: timestamp as u64,
            width: out_w as i32,
            height: out_h as i32,
            data: frame_data,
        }))),
        VideoFormat::xBGR => Ok(Some(Frame::XBGR(XBGRFrame {
            display_time: timestamp as u64,
            width: out_w as i32,
            height: out_h as i32,
            data: frame_data,
        }))),
        VideoFormat::BGRx => Ok(Some(Frame::BGRx(BGRxFrame {
            display_time: timestamp as u64,
            width: out_w as i32,
            height: out_h as i32,
            data: frame_data,
        }))),
        _ => Err(anyhow!("Unsupported frame format received")),
    }
}

struct PipewireCapture {
    _listener: pw::stream::StreamListener<ListenerUserData>,
    _stream: pw::stream::Stream,
    _core: pw::core::Core,
    _context: Context,
    mainloop: MainLoop,
}

fn start_pipewire_capturer(
    options: Options,
    tx: Sender<Result<Frame>>,
    stream_id: u32,
    stream_error: Arc<AtomicBool>,
) -> Result<PipewireCapture> {
    pw::init();

    let mainloop = MainLoop::new(None)?;
    let context = Context::new(&mainloop)?;
    let core = context.connect(None)?;

    let user_data = ListenerUserData {
        tx,
        format: Default::default(),
        stream_error,
    };

    let stream = pw::stream::Stream::new(
        &core,
        "scap",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    let listener = stream
        .add_local_listener_with_user_data(user_data)
        .state_changed(state_changed_callback)
        .param_changed(param_changed_callback)
        .process(process_callback)
        .register()?;

    let obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pw::spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pw::spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            pw::spa::param::video::VideoFormat::RGB,
            pw::spa::param::video::VideoFormat::RGBA,
            pw::spa::param::video::VideoFormat::RGBx,
            pw::spa::param::video::VideoFormat::BGRx,
        ),
        pw::spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle {
                // Default
                width: 128,
                height: 128,
            },
            pw::spa::utils::Rectangle {
                // Min
                width: 1,
                height: 1,
            },
            pw::spa::utils::Rectangle {
                // Max
                width: 4096,
                height: 4096,
            }
        ),
        pw::spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction {
                num: options.fps,
                denom: 1
            },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction {
                num: 1000,
                denom: 1
            }
        ),
    );

    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )?
    .0
    .into_inner();
    let (metas_values, crop_meta_values) = serialize_meta_params()?;

    let mut params = [
        pw::spa::pod::Pod::from_bytes(&values)
            .context("Not enough space in screen capture 'values' param.")?,
        pw::spa::pod::Pod::from_bytes(&metas_values)
            .context("Not enough space in screen capture 'metas_values' param.")?,
        pw::spa::pod::Pod::from_bytes(&crop_meta_values)
            .context("Not enough space in screen capture 'crop_meta_values' param.")?,
    ];

    stream.connect(
        Direction::Input,
        Some(stream_id),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    Ok(PipewireCapture {
        _listener: listener,
        _stream: stream,
        _core: core,
        _context: context,
        mainloop,
    })
}

// TODO: Format negotiation
fn pipewire_capturer(
    options: Options,
    tx: Sender<Result<Frame>>,
    ready_sender: &SyncSender<Result<()>>,
    stream_id: u32,
    capturer_state: Arc<AtomicU8>,
    stream_error: Arc<AtomicBool>,
) {
    let capture = match start_pipewire_capturer(options, tx, stream_id, stream_error.clone()) {
        Ok(capture) => {
            ready_sender.send(Ok(())).ok();
            capture
        }
        Err(err) => {
            ready_sender.send(Err(err)).ok();
            return;
        }
    };

    while capturer_state.load(std::sync::atomic::Ordering::Relaxed) == 0 {
        std::thread::sleep(Duration::from_millis(10));
    }

    let pw_loop = capture.mainloop.loop_();

    // User has called Capturer::start() and we start the main loop
    while capturer_state.load(std::sync::atomic::Ordering::Relaxed) == 1
        && /* If the stream state got changed to `Error`, we exit. TODO: tell user that we exited */
          !stream_error.load(std::sync::atomic::Ordering::Relaxed)
    {
        pw_loop.iterate(Duration::from_millis(100));
    }
}

pub struct WaylandCapturer {
    capturer_join_handle: Option<JoinHandle<()>>,
    capturer_state: Arc<AtomicU8>,
    // The pipewire stream is deleted when the connection is dropped.
    // That's why we keep it alive
    _connection: dbus::blocking::Connection,
}

impl WaylandCapturer {
    // TODO: Error handling
    pub fn new(options: &Options, tx: Sender<Result<Frame>>) -> Result<Self> {
        let connection = dbus::blocking::Connection::new_session()
            .context("Failed to create dbus connection")?;
        let stream_id = ScreenCastPortal::new(&connection)
            .source_types(options.portal_source_types)
            .show_cursor(options.show_cursor)
            .context("Unsupported screen capture cursor display mode")?
            .create_stream()
            .context("Failed to get screen capture stream")?
            .pw_node_id();

        // TODO: Fix this hack
        let options = options.clone();
        let (ready_sender, ready_recv) = sync_channel(1);
        let capturer_state = Arc::new(AtomicU8::new(0));
        let stream_error = Arc::new(AtomicBool::new(false));
        let thread_state = capturer_state.clone();
        let capturer_join_handle = std::thread::spawn(move || {
            pipewire_capturer(
                options,
                tx,
                &ready_sender,
                stream_id,
                thread_state,
                stream_error,
            )
        });

        match ready_recv.recv() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                return Err(anyhow!(err));
            }
            Err(RecvError) => {
                return Err(anyhow!(
                    "Wayland screen capture bug: stream unexpectedly dropped."
                ));
            }
        }

        Ok(Self {
            capturer_join_handle: Some(capturer_join_handle),
            capturer_state,
            _connection: connection,
        })
    }
}

impl LinuxCapturerImpl for WaylandCapturer {
    fn start_capture(&mut self) {
        self.capturer_state
            .store(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn stop_capture(&mut self) {
        self.capturer_state
            .store(2, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.capturer_join_handle.take() {
            match handle.join() {
                Ok(()) => {}
                Err(err) => log::error!("Failed to join Wayland screen capture thread: {:?}", err),
            }
        }
        self.capturer_state
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
}
