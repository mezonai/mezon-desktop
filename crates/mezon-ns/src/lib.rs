use std::ffi::CString;

use ort_sys as _;
use std::fmt;
use std::os::raw::{c_char, c_float, c_int, c_void};

mod resample;
pub use resample::Mezon48k;

pub const SAMPLE_RATE: usize = 16000;
pub const FRAME_SIZE: usize = 160; // 10ms frame at 16kHz
pub const FRAME_SIZE_48K: usize = 480;
pub static EMBEDDED_MODEL: &[u8] = include_bytes!("../assets/mezon_ns_asym_babble.onnx");

/// Configuration parameters for Mezon-NS engine.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MezonNSConfig {
    pub sample_rate: c_int,
    pub frame_size: c_int,
    pub attenuation_limit_db: c_float,
    pub num_threads: c_int,
    pub suppression_intensity: c_float,
    pub enable_noise_gate: c_int,
}

impl Default for MezonNSConfig {
    fn default() -> Self {
        let mut config = std::mem::MaybeUninit::<MezonNSConfig>::uninit();
        unsafe {
            mezon_ns_config_init(config.as_mut_ptr());
            config.assume_init()
        }
    }
}

// Low-level C FFI declarations matching mezon_ns.h
extern "C" {
    pub fn mezon_ns_config_init(config: *mut MezonNSConfig);
    pub fn mezon_ns_create(model_path: *const c_char, config: *const MezonNSConfig) -> *mut c_void;
    pub fn mezon_ns_create_from_memory(
        model_data: *const c_void,
        model_size: usize,
        config: *const MezonNSConfig,
    ) -> *mut c_void;
    pub fn mezon_ns_create_embedded(config: *const MezonNSConfig) -> *mut c_void;
    pub fn mezon_ns_has_embedded_model() -> c_int;
    pub fn mezon_ns_process_frame_float(
        engine: *mut c_void,
        in_frame: *const f32,
        out_frame: *mut f32,
    ) -> c_int;
    pub fn mezon_ns_process_frame_int16(
        engine: *mut c_void,
        in_frame: *const i16,
        out_frame: *mut i16,
    ) -> c_int;
    pub fn mezon_ns_set_suppression_intensity(engine: *mut c_void, value: c_float);
    pub fn mezon_ns_set_model_input_target_dbfs(engine: *mut c_void, target_dbfs: c_float);
    pub fn mezon_ns_reset(engine: *mut c_void);
    pub fn mezon_ns_destroy(engine: *mut c_void);
}

#[derive(Debug, Clone)]
pub enum MezonError {
    InitializationFailed,
    ProcessingFailed(i32),
    InvalidFrameSize { expected: usize, actual: usize },
    NullPointer,
    CStringError,
}

impl fmt::Display for MezonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MezonError::InitializationFailed => write!(f, "Failed to initialize Mezon-NS engine"),
            MezonError::ProcessingFailed(code) => {
                write!(f, "Audio processing failed with error code: {}", code)
            }
            MezonError::InvalidFrameSize { expected, actual } => {
                write!(
                    f,
                    "Invalid frame size: expected {} samples, got {}",
                    expected, actual
                )
            }
            MezonError::NullPointer => write!(f, "Encountered unexpected null pointer"),
            MezonError::CStringError => write!(f, "Invalid C string conversion"),
        }
    }
}

impl std::error::Error for MezonError {}

/// High-level safe Rust wrapper around Mezon-NS C++ Engine.
pub struct MezonNSEngine {
    ptr: *mut c_void,
}

// Engine owns its unique internal memory; can be transferred between threads.
unsafe impl Send for MezonNSEngine {}

impl MezonNSEngine {
    /// Help the model recognize quiet speech without raising transmitted PCM volume.
    pub fn set_model_input_target_dbfs(&mut self, target_dbfs: f32) {
        unsafe { mezon_ns_set_model_input_target_dbfs(self.ptr, target_dbfs) };
    }
    /// Check whether the native library was compiled with built-in embedded model weights.
    pub fn has_embedded_model() -> bool {
        true
    }

    /// Create engine using the bundled asym-babble model (zero filesystem access).
    pub fn create_embedded(config: Option<MezonNSConfig>) -> Result<Self, MezonError> {
        Self::create_from_memory(EMBEDDED_MODEL, config)
    }

    /// Create engine from an ONNX model file path.
    pub fn create(model_path: &str, config: Option<MezonNSConfig>) -> Result<Self, MezonError> {
        let c_path = CString::new(model_path).map_err(|_| MezonError::CStringError)?;
        let cfg = config.unwrap_or_default();
        let ptr = unsafe { mezon_ns_create(c_path.as_ptr(), &cfg) };
        if ptr.is_null() {
            Err(MezonError::InitializationFailed)
        } else {
            Ok(Self { ptr })
        }
    }

    /// Create engine from an in-memory byte slice of the ONNX model.
    pub fn create_from_memory(
        model_data: &[u8],
        config: Option<MezonNSConfig>,
    ) -> Result<Self, MezonError> {
        let cfg = config.unwrap_or_default();
        let ptr = unsafe {
            mezon_ns_create_from_memory(
                model_data.as_ptr() as *const c_void,
                model_data.len(),
                &cfg,
            )
        };
        if ptr.is_null() {
            Err(MezonError::InitializationFailed)
        } else {
            Ok(Self { ptr })
        }
    }

    /// Process a single 10ms frame (160 samples) of 16-bit PCM audio.
    ///
    /// `in_frame` and `out_frame` must both have length 160.
    /// In-place processing (`in_frame` and `out_frame` pointing to same buffer) is supported.
    pub fn process_frame_int16(
        &mut self,
        in_frame: &[i16],
        out_frame: &mut [i16],
    ) -> Result<(), MezonError> {
        if in_frame.len() != FRAME_SIZE {
            return Err(MezonError::InvalidFrameSize {
                expected: FRAME_SIZE,
                actual: in_frame.len(),
            });
        }
        if out_frame.len() != FRAME_SIZE {
            return Err(MezonError::InvalidFrameSize {
                expected: FRAME_SIZE,
                actual: out_frame.len(),
            });
        }

        let ret = unsafe {
            mezon_ns_process_frame_int16(self.ptr, in_frame.as_ptr(), out_frame.as_mut_ptr())
        };

        if ret == 0 {
            Ok(())
        } else {
            Err(MezonError::ProcessingFailed(ret))
        }
    }

    /// Process a single 10ms frame (160 samples) of 32-bit floating point audio [-1.0, 1.0].
    pub fn process_frame_float(
        &mut self,
        in_frame: &[f32],
        out_frame: &mut [f32],
    ) -> Result<(), MezonError> {
        if in_frame.len() != FRAME_SIZE {
            return Err(MezonError::InvalidFrameSize {
                expected: FRAME_SIZE,
                actual: in_frame.len(),
            });
        }
        if out_frame.len() != FRAME_SIZE {
            return Err(MezonError::InvalidFrameSize {
                expected: FRAME_SIZE,
                actual: out_frame.len(),
            });
        }

        let ret = unsafe {
            mezon_ns_process_frame_float(self.ptr, in_frame.as_ptr(), out_frame.as_mut_ptr())
        };

        if ret == 0 {
            Ok(())
        } else {
            Err(MezonError::ProcessingFailed(ret))
        }
    }

    /// Process an arbitrary-length stream of 16-bit PCM audio samples.
    ///
    /// Chunks audio into 160-sample 10ms frames. Zero-pads trailing partial frame if needed.
    pub fn process_stream_int16(
        &mut self,
        input: &[i16],
        output: &mut Vec<i16>,
    ) -> Result<(), MezonError> {
        output.clear();
        output.reserve(input.len());

        let mut in_chunk = [0i16; FRAME_SIZE];
        let mut out_chunk = [0i16; FRAME_SIZE];

        let mut offset = 0;
        while offset < input.len() {
            let chunk_len = std::cmp::min(FRAME_SIZE, input.len() - offset);
            in_chunk[..chunk_len].copy_from_slice(&input[offset..offset + chunk_len]);
            if chunk_len < FRAME_SIZE {
                in_chunk[chunk_len..].fill(0);
            }

            self.process_frame_int16(&in_chunk, &mut out_chunk)?;
            output.extend_from_slice(&out_chunk[..chunk_len]);
            offset += chunk_len;
        }

        Ok(())
    }

    /// Reset internal recurrent GRU states and overlap-add buffer history.
    pub fn reset(&mut self) {
        unsafe {
            mezon_ns_reset(self.ptr);
        }
    }

    pub fn set_suppression_intensity(&mut self, value: f32) {
        unsafe { mezon_ns_set_suppression_intensity(self.ptr, value) };
    }
}

impl Drop for MezonNSEngine {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                mezon_ns_destroy(self.ptr);
            }
            self.ptr = std::ptr::null_mut();
        }
    }
}
