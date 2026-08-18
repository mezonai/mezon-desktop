use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{E_FAIL, HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIAdapter;
use windows::Win32::Media::MediaFoundation::{
    CLSID_MFMediaEngineClassFactory, IMFAttributes, IMFDXGIDeviceManager, IMFMediaEngine,
    IMFMediaEngineClassFactory, IMFMediaEngineNotify, IMFMediaEngineNotify_Impl, IMFMediaType,
    IMFSample, IMFSourceReader, MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_DXGI_MANAGER,
    MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_MT_DEFAULT_STRIDE,
    MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR, MF_VERSION, MFCreateAttributes,
    MFCreateDXGIDeviceManager, MFCreateMediaType, MFCreateSourceReaderFromURL, MFMediaType_Video,
    MFSTARTUP_FULL, MFStartup, MFVideoFormat_RGB32,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Variant::VT_I8;
use windows::core::{BSTR, GUID, HSTRING, Interface, implement};

use crate::{PlayerError, VideoFrame, VideoProbe};

fn ensure_media_foundation() -> windows::core::Result<()> {
    static INIT: Once = Once::new();
    static READY: AtomicBool = AtomicBool::new(false);
    INIT.call_once(|| {
        if unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.is_ok() {
            READY.store(true, Ordering::SeqCst);
        }
    });
    if READY.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err(windows::core::Error::from(E_FAIL))
    }
}

#[implement(IMFMediaEngineNotify)]
struct EngineNotify {
    failed: Arc<AtomicBool>,
}

#[allow(non_snake_case)]
impl IMFMediaEngineNotify_Impl for EngineNotify_Impl {
    fn EventNotify(&self, event: u32, _param1: usize, _param2: u32) -> windows::core::Result<()> {
        if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 {
            self.failed.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

struct StagingTextures {
    width: u32,
    height: u32,
    render_target: ID3D11Texture2D,
    staging: ID3D11Texture2D,
}

pub struct PlayerImpl {
    engine: IMFMediaEngine,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    _dxgi_manager: IMFDXGIDeviceManager,
    _notify: IMFMediaEngineNotify,
    textures: RefCell<Option<StagingTextures>>,
    last_pts: Cell<i64>,
    max_size: Option<(u32, u32)>,
    failed: Arc<AtomicBool>,
}

impl PlayerImpl {
    pub fn open(url: &str, max_size: Option<(u32, u32)>) -> Result<Self, PlayerError> {
        if url.is_empty() {
            return Err(PlayerError::InvalidUrl);
        }
        Self::build(url, max_size).map_err(|error| {
            tracing::warn!(target: "mezon_video", ?error, "failed to open media foundation engine");
            PlayerError::Open
        })
    }

    fn build(url: &str, max_size: Option<(u32, u32)>) -> windows::core::Result<Self> {
        ensure_media_foundation()?;
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                None::<&IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                Some(&[
                    D3D_FEATURE_LEVEL_11_1,
                    D3D_FEATURE_LEVEL_11_0,
                    D3D_FEATURE_LEVEL_10_1,
                ]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
            let context = context.ok_or_else(|| windows::core::Error::from(E_FAIL))?;

            let multithread: ID3D11Multithread = context.cast()?;
            let _ = multithread.SetMultithreadProtected(true);

            let mut reset_token = 0u32;
            let mut dxgi_manager: Option<IMFDXGIDeviceManager> = None;
            MFCreateDXGIDeviceManager(&mut reset_token, &mut dxgi_manager)?;
            let dxgi_manager = dxgi_manager.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
            dxgi_manager.ResetDevice(&device, reset_token)?;

            let failed = Arc::new(AtomicBool::new(false));
            let notify: IMFMediaEngineNotify = EngineNotify {
                failed: failed.clone(),
            }
            .into();

            let mut attributes: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attributes, 4)?;
            let attributes = attributes.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
            attributes.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &dxgi_manager)?;
            attributes.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)?;
            attributes.SetUINT32(
                &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
                DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
            )?;

            let factory: IMFMediaEngineClassFactory =
                CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)?;
            let engine = factory.CreateInstance(0, &attributes)?;

            engine.SetSource(&BSTR::from(url))?;

            Ok(Self {
                engine,
                device,
                context,
                _dxgi_manager: dxgi_manager,
                _notify: notify,
                textures: RefCell::new(None),
                last_pts: Cell::new(0),
                max_size,
                failed,
            })
        }
    }

    pub fn copy_frame(&self) -> Option<VideoFrame> {
        unsafe {
            let pts = match self.engine.OnVideoStreamTick() {
                Ok(pts) => pts,
                Err(_) => return None,
            };
            if pts == 0 || pts == self.last_pts.get() {
                return None;
            }

            let mut width = 0u32;
            let mut height = 0u32;
            if self
                .engine
                .GetNativeVideoSize(Some(&mut width), Some(&mut height))
                .is_err()
            {
                return None;
            }
            if width == 0 || height == 0 {
                return None;
            }

            let (render_w, render_h) = match self.max_size {
                Some((max_w, max_h)) if max_w > 0 && max_h > 0 => {
                    (width.min(max_w), height.min(max_h))
                }
                _ => (width, height),
            };

            self.ensure_textures(render_w, render_h).ok()?;
            let textures = self.textures.borrow();
            let textures = textures.as_ref()?;

            let dst = RECT {
                left: 0,
                top: 0,
                right: render_w as i32,
                bottom: render_h as i32,
            };
            if self
                .engine
                .TransferVideoFrame(&textures.render_target, None, &dst, None)
                .is_err()
            {
                return None;
            }

            self.context
                .CopyResource(&textures.staging, &textures.render_target);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if self
                .context
                .Map(&textures.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .is_err()
            {
                return None;
            }

            let pitch = mapped.RowPitch as usize;
            let packed = if mapped.pData.is_null() {
                None
            } else {
                let region = pitch.saturating_mul(render_h as usize);
                let data = std::slice::from_raw_parts(mapped.pData as *const u8, region);
                crate::frame_util::pack_bgra_rows(data, pitch, render_w, render_h)
            };
            self.context.Unmap(&textures.staging, 0);

            let out = packed?;
            self.last_pts.set(pts);
            crate::render_frame::bgra_to_frame(render_w, render_h, out)
        }
    }

    fn ensure_textures(&self, width: u32, height: u32) -> windows::core::Result<()> {
        if let Some(existing) = self.textures.borrow().as_ref()
            && existing.width == width
            && existing.height == height
        {
            return Ok(());
        }
        let render_target = self.create_texture(width, height, false)?;
        let staging = self.create_texture(width, height, true)?;
        *self.textures.borrow_mut() = Some(StagingTextures {
            width,
            height,
            render_target,
            staging,
        });
        Ok(())
    }

    fn create_texture(
        &self,
        width: u32,
        height: u32,
        staging: bool,
    ) -> windows::core::Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: if staging {
                D3D11_USAGE_STAGING
            } else {
                D3D11_USAGE_DEFAULT
            },
            BindFlags: if staging {
                0
            } else {
                (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32
            },
            CPUAccessFlags: if staging {
                D3D11_CPU_ACCESS_READ.0 as u32
            } else {
                0
            },
            MiscFlags: 0,
        };
        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))?
        };
        texture.ok_or_else(|| windows::core::Error::from(E_FAIL))
    }

    pub fn play(&self) {
        unsafe {
            let _ = self.engine.Play();
        }
    }

    pub fn pause(&self) {
        unsafe {
            let _ = self.engine.Pause();
        }
    }

    pub fn is_playing(&self) -> bool {
        unsafe { !self.engine.IsPaused().as_bool() }
    }

    pub fn current_time(&self) -> f64 {
        let value = unsafe { self.engine.GetCurrentTime() };
        if value.is_finite() && value >= 0.0 {
            value
        } else {
            0.0
        }
    }

    pub fn duration(&self) -> f64 {
        let value = unsafe { self.engine.GetDuration() };
        if value.is_finite() && value >= 0.0 {
            value
        } else {
            0.0
        }
    }

    pub fn seek(&self, to_seconds: f64) {
        let target = if to_seconds.is_finite() && to_seconds >= 0.0 {
            to_seconds
        } else {
            0.0
        };
        unsafe {
            let _ = self.engine.SetCurrentTime(target);
        }
    }

    pub fn set_volume(&self, volume: f32) {
        let target = (volume as f64).clamp(0.0, 1.0);
        unsafe {
            let _ = self.engine.SetVolume(target);
        }
    }

    pub fn volume(&self) -> f32 {
        unsafe { self.engine.GetVolume() as f32 }
    }

    pub fn set_muted(&self, muted: bool) {
        unsafe {
            let _ = self.engine.SetMuted(muted);
        }
    }

    pub fn is_muted(&self) -> bool {
        unsafe { self.engine.GetMuted().as_bool() }
    }

    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }
}

impl Drop for PlayerImpl {
    fn drop(&mut self) {
        unsafe {
            let _ = self.engine.Shutdown();
        }
    }
}

const POSTER_TIME_HNS: i64 = 10_000_000;
const POSTER_MAX_READS: u32 = 32;

pub fn probe_video(path: &str, max_poster_edge: u32) -> Option<VideoProbe> {
    if path.is_empty() {
        return None;
    }
    ensure_media_foundation().ok()?;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    match unsafe { probe_with_source_reader(path, max_poster_edge) } {
        Ok(probe) => Some(probe),
        Err(error) => {
            tracing::warn!(target: "mezon_video", %error, "video probe failed");
            None
        }
    }
}

unsafe fn probe_with_source_reader(
    path: &str,
    max_poster_edge: u32,
) -> windows::core::Result<VideoProbe> {
    unsafe {
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 1)?;
        let attributes = attributes.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
        attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;

        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path), &attributes)?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
        reader.SetStreamSelection(stream, true)?;

        let requested = MFCreateMediaType()?;
        requested.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        requested.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        reader.SetCurrentMediaType(stream, None, &requested)?;

        let decoded = reader.GetCurrentMediaType(stream)?;
        let frame_size = decoded.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let width = (frame_size >> 32) as u32;
        let height = frame_size as u32;
        if width == 0 || height == 0 {
            return Err(windows::core::Error::from(E_FAIL));
        }

        let poster_jpeg = poster_frame(&reader, stream, &decoded, width, height, max_poster_edge);
        Ok(VideoProbe {
            width,
            height,
            poster_jpeg,
        })
    }
}

unsafe fn poster_frame(
    reader: &IMFSourceReader,
    stream: u32,
    decoded: &IMFMediaType,
    width: u32,
    height: u32,
    max_poster_edge: u32,
) -> Option<Vec<u8>> {
    unsafe {
        seek_to_poster_time(reader);
        let sample = read_first_frame(reader, stream)?;
        let buffer = sample.ConvertToContiguousBuffer().ok()?;
        let mut data = std::ptr::null_mut();
        let mut len = 0u32;
        buffer.Lock(&mut data, None, Some(&mut len)).ok()?;
        let (stride, bottom_up) = row_pitch(decoded, width);
        let jpeg = if data.is_null() {
            None
        } else {
            crate::poster::encode_poster_jpeg(
                std::slice::from_raw_parts(data, len as usize),
                width,
                height,
                stride,
                bottom_up,
                max_poster_edge,
            )
        };
        let _ = buffer.Unlock();
        jpeg
    }
}

unsafe fn seek_to_poster_time(reader: &IMFSourceReader) {
    unsafe {
        let mut position = PROPVARIANT::default();
        let value = &mut *position.Anonymous.Anonymous;
        value.vt = VT_I8;
        value.Anonymous.hVal = POSTER_TIME_HNS;
        let _ = reader.SetCurrentPosition(&GUID::zeroed(), &position);
    }
}

unsafe fn read_first_frame(reader: &IMFSourceReader, stream: u32) -> Option<IMFSample> {
    unsafe {
        for _ in 0..POSTER_MAX_READS {
            let mut flags = 0u32;
            let mut sample = None;
            reader
                .ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))
                .ok()?;
            if sample.is_some() {
                return sample;
            }
            let ended = flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0;
            let errored = flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0;
            if ended || errored {
                return None;
            }
        }
        None
    }
}

unsafe fn row_pitch(decoded: &IMFMediaType, width: u32) -> (usize, bool) {
    let tightly_packed = width as usize * 4;
    let Ok(declared) = (unsafe { decoded.GetUINT32(&MF_MT_DEFAULT_STRIDE) }) else {
        return (tightly_packed, false);
    };
    let signed = declared as i32;
    match signed.unsigned_abs() as usize {
        0 => (tightly_packed, false),
        pitch => (pitch, signed < 0),
    }
}
