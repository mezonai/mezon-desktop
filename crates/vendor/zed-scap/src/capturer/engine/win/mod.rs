use crate::{
    capturer::{Area, Options, Point, Resolution, Size},
    frame::{BGRAFrame, Frame, FrameType, RGBxFrame},
    targets::{self, Target},
};
use std::cmp;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
    frame::Frame as WCFrame,
    graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl},
    monitor::Monitor as WCMonitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings as WCSettings,
    },
    window::Window as WCWindow,
};

#[derive(Debug)]
struct Capturer {
    pub tx: mpsc::Sender<anyhow::Result<Frame>>,
    pub crop: Option<Area>,
}

#[derive(Clone)]
enum Settings {
    Window(WCSettings<FlagStruct, WCWindow>),
    Display(WCSettings<FlagStruct, WCMonitor>),
}

pub struct WCStream {
    settings: Settings,
    capture_control: Option<CaptureControl<Capturer, Box<dyn std::error::Error + Send + Sync>>>,
    error_tx: mpsc::Sender<anyhow::Result<Frame>>,
}

impl GraphicsCaptureApiHandler for Capturer {
    type Flags = FlagStruct;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(context: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            tx: context.flags.tx,
            crop: context.flags.crop,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut WCFrame,
        _: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let color_format = frame.color_format();
        let bytes_per_pixel = match color_format {
            ColorFormat::Rgba16F => return Err(self.fail("Rgba16F is not yet supported".into())),
            ColorFormat::Rgba8 | ColorFormat::Bgra8 => 4,
        };

        let (width, height, data) =
            match copy_frame_region(frame, self.crop.as_ref(), bytes_per_pixel) {
                Ok(copied) => copied,
                Err(e) => return Err(self.fail(format!("frame copy failed: {e}"))),
            };

        let display_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        let frame = if matches!(color_format, ColorFormat::Rgba8) {
            Frame::RGBx(RGBxFrame {
                display_time,
                width,
                height,
                data,
            })
        } else {
            Frame::BGRA(BGRAFrame {
                display_time,
                width,
                height,
                data,
            })
        };

        Ok(self.tx.send(Ok(frame))?)
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        log::debug!("Screen capture stream closed.");
        let _ = self.tx.send(Err(anyhow::anyhow!("capture source closed")));
        Ok(())
    }
}

impl Capturer {
    fn fail(&self, message: String) -> Box<dyn std::error::Error + Send + Sync> {
        let _ = self.tx.send(Err(anyhow::anyhow!("{message}")));
        message.into()
    }
}

fn copy_frame_region(
    frame: &WCFrame,
    crop: Option<&Area>,
    bytes_per_pixel: usize,
) -> Result<(i32, i32, Vec<u8>), Box<dyn std::error::Error + Send + Sync>> {
    let source = unsafe { frame.as_raw_texture() };
    let mut source_desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { source.GetDesc(&mut source_desc) };

    let region = clamp_to_frame(crop, source_desc.Width, source_desc.Height);
    if region.right <= region.left || region.bottom <= region.top {
        return Err("capture frame has no visible area".into());
    }
    let width = region.right - region.left;
    let height = region.bottom - region.top;

    let staging_desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: source_desc.Format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };

    let device = unsafe { source.GetDevice() }?;
    let context = unsafe { device.GetImmediateContext() }?;

    let mut staging = None;
    unsafe { device.CreateTexture2D(&staging_desc, None, Some(&mut staging)) }?;
    let staging = staging.ok_or("staging texture was not created")?;

    unsafe {
        context.CopySubresourceRegion(&staging, 0, 0, 0, 0, source, 0, Some(&region));
    }

    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }?;

    let row_bytes = width as usize * bytes_per_pixel;
    let mut pixels = vec![0u8; row_bytes * height as usize];
    if !mapped.pData.is_null() {
        let row_pitch = mapped.RowPitch as usize;
        let copied = row_bytes.min(row_pitch);
        for (y, row) in pixels.chunks_exact_mut(row_bytes).enumerate() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    mapped.pData.cast::<u8>().add(y * row_pitch),
                    row.as_mut_ptr(),
                    copied,
                );
            }
        }
    }

    unsafe { context.Unmap(&staging, 0) };

    Ok((width as i32, height as i32, pixels))
}

fn clamp_to_frame(crop: Option<&Area>, frame_width: u32, frame_height: u32) -> D3D11_BOX {
    let (left, top, right, bottom) = match crop {
        Some(crop) => (
            clamp_axis(crop.origin.x, frame_width),
            clamp_axis(crop.origin.y, frame_height),
            clamp_axis(crop.origin.x + crop.size.width, frame_width),
            clamp_axis(crop.origin.y + crop.size.height, frame_height),
        ),
        None => (0, 0, frame_width, frame_height),
    };

    D3D11_BOX {
        left,
        top,
        front: 0,
        right,
        bottom,
        back: 1,
    }
}

fn clamp_axis(value: f64, limit: u32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    (value as u32).min(limit)
}

impl WCStream {
    pub fn start_capture(&mut self) {
        let cc = match &self.settings {
            Settings::Display(st) => Capturer::start_free_threaded(st.to_owned()),
            Settings::Window(st) => Capturer::start_free_threaded(st.to_owned()),
        };

        match cc {
            Ok(cc) => self.capture_control = Some(cc),
            Err(e) => {
                log::error!("failed to start screen capture: {e}");
                let _ = self
                    .error_tx
                    .send(Err(anyhow::anyhow!("start capture: {e}")));
            }
        }
    }

    pub fn stop_capture(&mut self) {
        if let Some(capture_control) = self.capture_control.take() {
            let _ = capture_control.stop();
        }
    }
}

#[derive(Clone, Debug)]
struct FlagStruct {
    pub tx: mpsc::Sender<anyhow::Result<Frame>>,
    pub crop: Option<Area>,
}

pub fn create_capturer(options: &Options, tx: mpsc::Sender<anyhow::Result<Frame>>) -> (WCStream, Target) {
    let target = options.target.clone().unwrap_or_else(|| {
        Target::Display(targets::get_main_display().expect("Failed to get main display"))
    });

    let color_format = match options.output_type {
        FrameType::BGRAFrame => ColorFormat::Bgra8,
        _ => ColorFormat::Rgba8,
    };

    let show_cursor = if GraphicsCaptureApi::is_cursor_settings_supported().unwrap_or(false) {
        match options.show_cursor {
            true => CursorCaptureSettings::WithCursor,
            false => CursorCaptureSettings::WithoutCursor,
        }
    } else {
        CursorCaptureSettings::Default
    };

    let error_tx = tx.clone();

    let settings = match target.clone() {
        Target::Display(display) => Settings::Display(WCSettings::new(
            WCMonitor::from_raw_hmonitor(display.raw_handle.0),
            show_cursor,
            DrawBorderSettings::Default,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            color_format,
            FlagStruct {
                tx,
                crop: Some(get_crop_area(options)),
            },
        )),
        Target::Window(window) => Settings::Window(WCSettings::new(
            WCWindow::from_raw_hwnd(window.raw_handle.0),
            show_cursor,
            DrawBorderSettings::Default,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            color_format,
            FlagStruct {
                tx,
                crop: options.crop_area.as_ref().map(|_| get_crop_area(options)),
            },
        )),
    };

    (WCStream {
        settings,
        capture_control: None,
        error_tx,
    }, target)
}

pub fn get_output_frame_size(options: &Options) -> [u32; 2] {
    let crop_area = get_crop_area(options);

    let mut output_width = (crop_area.size.width) as u32;
    let mut output_height = (crop_area.size.height) as u32;

    match options.output_resolution {
        Resolution::Captured => {}
        _ => {
            let [resolved_width, resolved_height] = options
                .output_resolution
                .value((crop_area.size.width as f32) / (crop_area.size.height as f32));
            output_width = cmp::min(output_width, resolved_width);
            output_height = cmp::min(output_height, resolved_height);
        }
    }

    output_width -= output_width % 2;
    output_height -= output_height % 2;

    [output_width, output_height]
}

fn get_absolute_value(value: f64, scale_factor: f64) -> f64 {
    let value = (value * scale_factor).floor();
    value + value % 2.0
}

pub fn get_crop_area(options: &Options) -> Area {
    let target = options.target.clone().unwrap_or_else(|| {
        Target::Display(targets::get_main_display().expect("Failed to get main display"))
    });

    let (width, height) = targets::get_target_dimensions(&target);

    let scale_factor = targets::get_scale_factor(&target);
    options
        .crop_area
        .as_ref()
        .map(|val| {
            Area {
                origin: Point {
                    x: get_absolute_value(val.origin.x, scale_factor),
                    y: get_absolute_value(val.origin.y, scale_factor),
                },
                size: Size {
                    width: get_absolute_value(val.size.width, scale_factor),
                    height: get_absolute_value(val.size.height, scale_factor),
                },
            }
        })
        .unwrap_or_else(|| Area {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size {
                width: width as f64,
                height: height as f64,
            },
        })
}
