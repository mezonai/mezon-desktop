use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::ptr::NonNull;
use std::sync::Once;

use parking_lot::Mutex;
use windows::Win32::Foundation::{
    CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_NOTIMPL, E_POINTER,
};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, IMFMediaBuffer, IMFMediaEvent, IMFMediaType, IMFSample,
    IMFTransform, IMFTransform_Impl, MF_E_INVALIDMEDIATYPE, MF_E_INVALIDSTREAMNUMBER,
    MF_E_NO_MORE_TYPES, MF_E_NOTACCEPTING, MF_E_TRANSFORM_CANNOT_CHANGE_MEDIATYPE_WHILE_PROCESSING,
    MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_TYPE_NOT_SET, MF_MT_ALL_SAMPLES_INDEPENDENT,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
    MF_MT_AUDIO_CHANNEL_MASK, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_MAJOR_TYPE, MF_MT_MPEG4_SAMPLE_DESCRIPTION, MF_MT_SUBTYPE, MF_MT_USER_DATA,
    MFAudioFormat_Float, MFAudioFormat_Opus, MFAudioFormat_PCM, MFCreateAttributes,
    MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Audio,
    MFSampleExtension_Discontinuity, MFT_CATEGORY_AUDIO_DECODER, MFT_ENUM_FLAG_ASYNCMFT,
    MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT,
    MFT_INPUT_STATUS_ACCEPT_DATA, MFT_INPUT_STREAM_INFO, MFT_INPUT_STREAM_SINGLE_SAMPLE_PER_BUFFER,
    MFT_INPUT_STREAM_WHOLE_SAMPLES, MFT_MESSAGE_COMMAND_FLUSH, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_MESSAGE_TYPE, MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STATUS_SAMPLE_READY,
    MFT_OUTPUT_STREAM_INFO, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFT_OUTPUT_STREAM_WHOLE_SAMPLES,
    MFT_REGISTER_TYPE_INFO, MFT_SET_TYPE_TEST_ONLY, MFTEnumEx, MFTRegisterLocal,
};
use windows::Win32::System::Com::{
    COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize, IClassFactory,
    IClassFactory_Impl,
};
use windows::core::{BOOL, GUID, IUnknown, Interface, Ref, Result, implement, w};

pub(crate) const OPUS_FOURCC_SUBTYPE: GUID =
    GUID::from_u128(0x7375704f_0000_0010_8000_00aa00389b71);
const SAMPLE_RATE: u32 = 48_000;
const MAX_FRAMES_PER_PACKET: usize = 5_760;
const HNS_PER_SECOND: i64 = 10_000_000;
const FORCE_ENV: &str = "MEZON_OPUS_DECODER";
const CONCEAL_STEP: usize = 120;

pub(crate) fn register_opus_decoder() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let force = std::env::var(FORCE_ENV).is_ok_and(|value| value == "in-app");
        if force {
            register_logged(&[OPUS_FOURCC_SUBTYPE, MFAudioFormat_Opus], true);
            return;
        }
        register_logged(&[OPUS_FOURCC_SUBTYPE], false);
        let spawned = std::thread::Builder::new()
            .name("mezon-opus-mft".into())
            .spawn(|| {
                let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
                let started = std::time::Instant::now();
                match system_decoder_exists(MFAudioFormat_Opus) {
                    Ok(system_decoder) => {
                        tracing::info!(
                            target: "mezon_video",
                            system_decoder,
                            elapsed_ms = started.elapsed().as_millis() as u64,
                            "checked for a windows opus decoder"
                        );
                        if !system_decoder {
                            register_logged(&[MFAudioFormat_Opus], false);
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            target: "mezon_video",
                            ?error,
                            "could not check for a windows opus decoder; using the in-app one"
                        );
                        register_logged(&[MFAudioFormat_Opus], false);
                    }
                }
                if com {
                    unsafe { CoUninitialize() };
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(
                target: "mezon_video",
                %error,
                "could not start the opus decoder check"
            );
        }
    });
}

fn register_logged(subtypes: &[GUID], force: bool) {
    let standard_opus = subtypes.contains(&MFAudioFormat_Opus);
    match register_for(subtypes) {
        Ok(()) => tracing::info!(
            target: "mezon_video",
            standard_opus,
            force,
            "registered in-app opus decoder"
        ),
        Err(error) => tracing::warn!(
            target: "mezon_video",
            ?error,
            standard_opus,
            "failed to register in-app opus decoder"
        ),
    }
}

pub(crate) fn system_decoder_exists(subtype: GUID) -> Result<bool> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Audio,
        guidSubtype: subtype,
    };
    let flags = MFT_ENUM_FLAG_SYNCMFT
        | MFT_ENUM_FLAG_ASYNCMFT
        | MFT_ENUM_FLAG_HARDWARE
        | MFT_ENUM_FLAG_SORTANDFILTER;
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    let enumerated = unsafe {
        MFTEnumEx(
            MFT_CATEGORY_AUDIO_DECODER,
            flags,
            Some(&input as *const _),
            None,
            &mut activates,
            &mut count,
        )
    };
    if !activates.is_null() {
        unsafe {
            for index in 0..count as usize {
                std::ptr::drop_in_place(activates.add(index));
            }
            CoTaskMemFree(Some(activates as *const c_void));
        }
    }
    enumerated?;
    Ok(count > 0)
}

fn register_for(subtypes: &[GUID]) -> Result<()> {
    let inputs: Vec<MFT_REGISTER_TYPE_INFO> = subtypes
        .iter()
        .map(|subtype| MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Audio,
            guidSubtype: *subtype,
        })
        .collect();
    let outputs = [SampleFormat::Float, SampleFormat::Pcm16].map(|format| MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Audio,
        guidSubtype: format.subtype(),
    });
    let factory: IClassFactory = OpusDecoderFactory.into();
    unsafe {
        MFTRegisterLocal(
            &factory,
            &MFT_CATEGORY_AUDIO_DECODER,
            w!("Mezon Opus Decoder"),
            0,
            Some(&inputs),
            Some(&outputs),
        )
    }
}

#[implement(IClassFactory)]
struct OpusDecoderFactory;

#[allow(non_snake_case)]
impl IClassFactory_Impl for OpusDecoderFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, IUnknown>,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> Result<()> {
        if object.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *object = std::ptr::null_mut() };
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let transform: IMFTransform = OpusDecoder::new()?.into();
        unsafe { transform.query(iid, object) }.ok()
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SampleFormat {
    Float,
    Pcm16,
}

impl SampleFormat {
    fn subtype(self) -> GUID {
        match self {
            Self::Float => MFAudioFormat_Float,
            Self::Pcm16 => MFAudioFormat_PCM,
        }
    }

    fn bits(self) -> u32 {
        match self {
            Self::Float => 32,
            Self::Pcm16 => 16,
        }
    }

    fn from_subtype(subtype: GUID) -> Option<Self> {
        [Self::Float, Self::Pcm16]
            .into_iter()
            .find(|format| format.subtype() == subtype)
    }

    fn encode(self, pcm: &[f32]) -> Vec<u8> {
        match self {
            Self::Float => pcm.iter().flat_map(|value| value.to_le_bytes()).collect(),
            Self::Pcm16 => pcm
                .iter()
                .flat_map(|value| {
                    let scaled = (value.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
                    scaled.to_le_bytes()
                })
                .collect(),
        }
    }
}

struct OpusHandle(NonNull<unsafe_libopus::OpusDecoder>);

unsafe impl Send for OpusHandle {}

impl OpusHandle {
    fn new(channels: u32) -> Result<Self> {
        let mut error = 0i32;
        let raw = unsafe {
            unsafe_libopus::opus_decoder_create(SAMPLE_RATE as i32, channels as i32, &mut error)
        };
        match NonNull::new(raw) {
            Some(handle) if error == 0 => Ok(Self(handle)),
            Some(handle) => {
                unsafe { unsafe_libopus::opus_decoder_destroy(handle.as_ptr()) };
                Err(E_FAIL.into())
            }
            None => Err(E_FAIL.into()),
        }
    }

    fn decode(&mut self, packet: &[u8], pcm: &mut [f32], channels: usize) -> i32 {
        unsafe {
            unsafe_libopus::opus_decode_float(
                self.0.as_ptr(),
                packet.as_ptr(),
                packet.len() as i32,
                pcm.as_mut_ptr(),
                (pcm.len() / channels) as i32,
                0,
            )
        }
    }

    fn conceal(&mut self, pcm: &mut [f32], frames: usize) -> i32 {
        unsafe {
            unsafe_libopus::opus_decode_float(
                self.0.as_ptr(),
                std::ptr::null(),
                0,
                pcm.as_mut_ptr(),
                frames as i32,
                0,
            )
        }
    }
}

fn concealment_frames(duration: Option<i64>, last_frames: usize) -> usize {
    let frames = duration
        .filter(|duration| *duration > 0)
        .map(|duration| (duration * SAMPLE_RATE as i64 / HNS_PER_SECOND) as usize)
        .unwrap_or(last_frames);
    ((frames + CONCEAL_STEP / 2) / CONCEAL_STEP * CONCEAL_STEP).min(MAX_FRAMES_PER_PACKET)
}

impl Drop for OpusHandle {
    fn drop(&mut self) {
        unsafe { unsafe_libopus::opus_decoder_destroy(self.0.as_ptr()) };
    }
}

struct DecodedChunk {
    bytes: Vec<u8>,
    time: i64,
    duration: i64,
    discontinuity: bool,
}

#[derive(Default)]
struct DecoderState {
    input_type: Option<IMFMediaType>,
    output_type: Option<IMFMediaType>,
    output_format: Option<SampleFormat>,
    channels: u32,
    decoder: Option<OpusHandle>,
    pending: Option<DecodedChunk>,
    next_time: Option<i64>,
    decoded_packets: u64,
    failed_packets: u64,
    pcm: Vec<f32>,
    last_frames: usize,
}

impl DecoderState {
    fn reset_stream(&mut self) {
        self.pending = None;
        self.next_time = None;
        self.decoder = None;
        self.last_frames = 0;
    }

    fn decode_packet(&mut self, packet: &[u8], duration: Option<i64>) -> Result<usize> {
        let channels = self.channels as usize;
        if self.decoder.is_none() {
            self.decoder = Some(OpusHandle::new(self.channels)?);
        }
        self.pcm.resize(MAX_FRAMES_PER_PACKET * channels, 0.0);
        let decoder = self.decoder.as_mut().ok_or(E_FAIL)?;
        if !packet.is_empty() {
            let frames = decoder.decode(packet, &mut self.pcm, channels);
            if frames >= 0 {
                self.decoded_packets += 1;
                if self.decoded_packets == 1 {
                    tracing::info!(
                        target: "mezon_video",
                        frames,
                        channels,
                        "in-app opus decoder produced its first audio"
                    );
                }
                self.last_frames = frames as usize;
                return Ok(frames as usize);
            }
            self.failed_packets += 1;
            if self.failed_packets == 1 {
                tracing::warn!(
                    target: "mezon_video",
                    code = frames,
                    bytes = packet.len(),
                    "in-app opus decoder rejected a packet; concealing it"
                );
            }
        }
        let frames = concealment_frames(duration, self.last_frames);
        if frames > 0 && decoder.conceal(&mut self.pcm, frames) < 0 {
            self.pcm[..frames * channels].fill(0.0);
        }
        Ok(frames)
    }
}

#[implement(IMFTransform)]
struct OpusDecoder {
    attributes: IMFAttributes,
    state: Mutex<DecoderState>,
}

impl OpusDecoder {
    fn new() -> Result<Self> {
        let mut attributes: Option<IMFAttributes> = None;
        unsafe { MFCreateAttributes(&mut attributes, 1)? };
        Ok(Self {
            attributes: attributes.ok_or(E_FAIL)?,
            state: Mutex::new(DecoderState::default()),
        })
    }
}

fn is_opus_subtype(subtype: GUID) -> bool {
    subtype == MFAudioFormat_Opus || subtype == OPUS_FOURCC_SUBTYPE
}

fn check_stream(stream: u32) -> Result<()> {
    if stream == 0 {
        Ok(())
    } else {
        Err(MF_E_INVALIDSTREAMNUMBER.into())
    }
}

fn blob(media_type: &IMFMediaType, key: &GUID) -> Option<Vec<u8>> {
    let size = unsafe { media_type.GetBlobSize(key) }.ok()?;
    let mut bytes = vec![0u8; size as usize];
    unsafe { media_type.GetBlob(key, &mut bytes, None) }.ok()?;
    Some(bytes)
}

pub(crate) fn channels_from_codec_config(config: &[u8]) -> Option<u32> {
    if let Some(at) = config.windows(8).position(|window| window == b"OpusHead") {
        return config.get(at + 9).map(|channels| u32::from(*channels));
    }
    let at = config.windows(4).position(|window| window == b"dOps")?;
    config.get(at + 5).map(|channels| u32::from(*channels))
}

fn input_channels(media_type: &IMFMediaType) -> Result<u32> {
    let major = unsafe { media_type.GetGUID(&MF_MT_MAJOR_TYPE)? };
    let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE)? };
    if major != MFMediaType_Audio || !is_opus_subtype(subtype) {
        return Err(MF_E_INVALIDMEDIATYPE.into());
    }
    let channels = unsafe { media_type.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS) }
        .ok()
        .filter(|channels| *channels > 0)
        .or_else(|| {
            [MF_MT_USER_DATA, MF_MT_MPEG4_SAMPLE_DESCRIPTION]
                .iter()
                .filter_map(|key| blob(media_type, key))
                .find_map(|config| channels_from_codec_config(&config))
        })
        .unwrap_or(2);
    if channels == 1 || channels == 2 {
        Ok(channels)
    } else {
        Err(MF_E_INVALIDMEDIATYPE.into())
    }
}

fn partial_input_type(subtype: GUID) -> Result<IMFMediaType> {
    let media_type = unsafe { MFCreateMediaType()? };
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
    }
    Ok(media_type)
}

fn output_media_type(format: SampleFormat, channels: u32) -> Result<IMFMediaType> {
    let block_align = channels * format.bits() / 8;
    let channel_mask = if channels == 1 { 0x4 } else { 0x3 };
    let media_type = unsafe { MFCreateMediaType()? };
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &format.subtype())?;
        media_type.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)?;
        media_type.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)?;
        media_type.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, format.bits())?;
        media_type.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block_align)?;
        media_type.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, block_align * SAMPLE_RATE)?;
        media_type.SetUINT32(&MF_MT_AUDIO_CHANNEL_MASK, channel_mask)?;
        media_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
    }
    Ok(media_type)
}

fn output_format(media_type: &IMFMediaType, channels: u32) -> Result<SampleFormat> {
    let major = unsafe { media_type.GetGUID(&MF_MT_MAJOR_TYPE)? };
    let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE)? };
    let format = SampleFormat::from_subtype(subtype)
        .filter(|_| major == MFMediaType_Audio)
        .ok_or(MF_E_INVALIDMEDIATYPE)?;
    let matches = |key: &GUID, expected: u32| {
        unsafe { media_type.GetUINT32(key) }.map_or(true, |value| value == expected)
    };
    if matches(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)
        && matches(&MF_MT_AUDIO_NUM_CHANNELS, channels)
        && matches(&MF_MT_AUDIO_BITS_PER_SAMPLE, format.bits())
    {
        Ok(format)
    } else {
        Err(MF_E_INVALIDMEDIATYPE.into())
    }
}

fn sample_bytes(sample: &IMFSample) -> Result<Vec<u8>> {
    let buffer = unsafe { sample.ConvertToContiguousBuffer()? };
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut length = 0u32;
    unsafe { buffer.Lock(&mut data, None, Some(&mut length))? };
    let bytes = if data.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data, length as usize) }.to_vec()
    };
    unsafe { buffer.Unlock()? };
    Ok(bytes)
}

fn output_sample(chunk: &DecodedChunk) -> Result<IMFSample> {
    let length = chunk.bytes.len() as u32;
    let buffer: IMFMediaBuffer = unsafe { MFCreateMemoryBuffer(length.max(1))? };
    let mut data: *mut u8 = std::ptr::null_mut();
    unsafe { buffer.Lock(&mut data, None, None)? };
    if !data.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(chunk.bytes.as_ptr(), data, chunk.bytes.len()) };
    }
    unsafe {
        buffer.Unlock()?;
        buffer.SetCurrentLength(length)?;
    }
    let sample = unsafe { MFCreateSample()? };
    unsafe {
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(chunk.time)?;
        sample.SetSampleDuration(chunk.duration)?;
        if chunk.discontinuity {
            sample.SetUINT32(&MFSampleExtension_Discontinuity, 1)?;
        }
    }
    Ok(sample)
}

#[allow(non_snake_case)]
impl IMFTransform_Impl for OpusDecoder_Impl {
    fn GetStreamLimits(
        &self,
        input_minimum: *mut u32,
        input_maximum: *mut u32,
        output_minimum: *mut u32,
        output_maximum: *mut u32,
    ) -> Result<()> {
        for limit in [input_minimum, input_maximum, output_minimum, output_maximum] {
            if limit.is_null() {
                return Err(E_POINTER.into());
            }
            unsafe { *limit = 1 };
        }
        Ok(())
    }

    fn GetStreamCount(&self, input_streams: *mut u32, output_streams: *mut u32) -> Result<()> {
        if input_streams.is_null() || output_streams.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *input_streams = 1;
            *output_streams = 1;
        }
        Ok(())
    }

    fn GetStreamIDs(&self, _: u32, _: *mut u32, _: u32, _: *mut u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn GetInputStreamInfo(&self, stream: u32, info: *mut MFT_INPUT_STREAM_INFO) -> Result<()> {
        check_stream(stream)?;
        if info.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *info = MFT_INPUT_STREAM_INFO {
                hnsMaxLatency: 0,
                dwFlags: (MFT_INPUT_STREAM_WHOLE_SAMPLES.0
                    | MFT_INPUT_STREAM_SINGLE_SAMPLE_PER_BUFFER.0) as u32,
                cbSize: 0,
                cbMaxLookahead: 0,
                cbAlignment: 0,
            };
        }
        Ok(())
    }

    fn GetOutputStreamInfo(&self, stream: u32) -> Result<MFT_OUTPUT_STREAM_INFO> {
        check_stream(stream)?;
        let channels = self.state.lock().channels.max(1);
        Ok(MFT_OUTPUT_STREAM_INFO {
            dwFlags: (MFT_OUTPUT_STREAM_WHOLE_SAMPLES.0 | MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0)
                as u32,
            cbSize: MAX_FRAMES_PER_PACKET as u32 * channels * 4,
            cbAlignment: 0,
        })
    }

    fn GetAttributes(&self) -> Result<IMFAttributes> {
        Ok(self.attributes.clone())
    }

    fn GetInputStreamAttributes(&self, _: u32) -> Result<IMFAttributes> {
        Err(E_NOTIMPL.into())
    }

    fn GetOutputStreamAttributes(&self, _: u32) -> Result<IMFAttributes> {
        Err(E_NOTIMPL.into())
    }

    fn DeleteInputStream(&self, _: u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn AddInputStreams(&self, _: u32, _: *const u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn GetInputAvailableType(&self, stream: u32, index: u32) -> Result<IMFMediaType> {
        check_stream(stream)?;
        match index {
            0 => partial_input_type(MFAudioFormat_Opus),
            1 => partial_input_type(OPUS_FOURCC_SUBTYPE),
            _ => Err(MF_E_NO_MORE_TYPES.into()),
        }
    }

    fn GetOutputAvailableType(&self, stream: u32, index: u32) -> Result<IMFMediaType> {
        check_stream(stream)?;
        let state = self.state.lock();
        if state.input_type.is_none() {
            return Err(MF_E_TRANSFORM_TYPE_NOT_SET.into());
        }
        match index {
            0 => output_media_type(SampleFormat::Float, state.channels),
            1 => output_media_type(SampleFormat::Pcm16, state.channels),
            _ => Err(MF_E_NO_MORE_TYPES.into()),
        }
    }

    fn SetInputType(
        &self,
        stream: u32,
        media_type: Ref<'_, IMFMediaType>,
        flags: u32,
    ) -> Result<()> {
        check_stream(stream)?;
        let test_only = flags & MFT_SET_TYPE_TEST_ONLY.0 as u32 != 0;
        let mut state = self.state.lock();
        let Some(media_type) = media_type.as_ref() else {
            if !test_only {
                *state = DecoderState::default();
            }
            return Ok(());
        };
        let channels = input_channels(media_type)?;
        if test_only {
            return Ok(());
        }
        if state.pending.is_some() {
            return Err(MF_E_TRANSFORM_CANNOT_CHANGE_MEDIATYPE_WHILE_PROCESSING.into());
        }
        let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE)? };
        if state.channels != channels {
            state.output_type = None;
            state.output_format = None;
        }
        state.reset_stream();
        state.channels = channels;
        state.input_type = Some(media_type.clone());
        tracing::info!(
            target: "mezon_video",
            standard_subtype = subtype == MFAudioFormat_Opus,
            channels,
            "in-app opus decoder accepted an audio stream"
        );
        Ok(())
    }

    fn SetOutputType(
        &self,
        stream: u32,
        media_type: Ref<'_, IMFMediaType>,
        flags: u32,
    ) -> Result<()> {
        check_stream(stream)?;
        let test_only = flags & MFT_SET_TYPE_TEST_ONLY.0 as u32 != 0;
        let mut state = self.state.lock();
        if state.input_type.is_none() {
            return Err(MF_E_TRANSFORM_TYPE_NOT_SET.into());
        }
        let Some(media_type) = media_type.as_ref() else {
            if !test_only {
                state.output_type = None;
                state.output_format = None;
            }
            return Ok(());
        };
        let format = output_format(media_type, state.channels)?;
        if test_only {
            return Ok(());
        }
        if state.pending.is_some() {
            return Err(MF_E_TRANSFORM_CANNOT_CHANGE_MEDIATYPE_WHILE_PROCESSING.into());
        }
        state.output_type = Some(media_type.clone());
        state.output_format = Some(format);
        Ok(())
    }

    fn GetInputCurrentType(&self, stream: u32) -> Result<IMFMediaType> {
        check_stream(stream)?;
        self.state
            .lock()
            .input_type
            .clone()
            .ok_or_else(|| MF_E_TRANSFORM_TYPE_NOT_SET.into())
    }

    fn GetOutputCurrentType(&self, stream: u32) -> Result<IMFMediaType> {
        check_stream(stream)?;
        self.state
            .lock()
            .output_type
            .clone()
            .ok_or_else(|| MF_E_TRANSFORM_TYPE_NOT_SET.into())
    }

    fn GetInputStatus(&self, stream: u32) -> Result<u32> {
        check_stream(stream)?;
        let state = self.state.lock();
        if state.input_type.is_none() {
            return Err(MF_E_TRANSFORM_TYPE_NOT_SET.into());
        }
        Ok(if state.pending.is_none() {
            MFT_INPUT_STATUS_ACCEPT_DATA.0 as u32
        } else {
            0
        })
    }

    fn GetOutputStatus(&self) -> Result<u32> {
        Ok(if self.state.lock().pending.is_some() {
            MFT_OUTPUT_STATUS_SAMPLE_READY.0 as u32
        } else {
            0
        })
    }

    fn SetOutputBounds(&self, _: i64, _: i64) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn ProcessEvent(&self, _: u32, _: Ref<'_, IMFMediaEvent>) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn ProcessMessage(&self, message: MFT_MESSAGE_TYPE, _: usize) -> Result<()> {
        if message == MFT_MESSAGE_COMMAND_FLUSH || message == MFT_MESSAGE_NOTIFY_START_OF_STREAM {
            self.state.lock().reset_stream();
        }
        Ok(())
    }

    fn ProcessInput(&self, stream: u32, sample: Ref<'_, IMFSample>, _: u32) -> Result<()> {
        check_stream(stream)?;
        let sample = sample.ok()?;
        let mut state = self.state.lock();
        let Some(format) = state.output_format.filter(|_| state.input_type.is_some()) else {
            return Err(MF_E_TRANSFORM_TYPE_NOT_SET.into());
        };
        if state.pending.is_some() {
            return Err(MF_E_NOTACCEPTING.into());
        }
        let packet = sample_bytes(sample)?;
        let discontinuity =
            unsafe { sample.GetUINT32(&MFSampleExtension_Discontinuity) }.unwrap_or(0) != 0;
        if discontinuity {
            state.decoder = None;
        }
        let input_duration = unsafe { sample.GetSampleDuration() }.ok();
        let time = unsafe { sample.GetSampleTime() }
            .ok()
            .or(state.next_time)
            .unwrap_or(0);
        let frames = state.decode_packet(&packet, input_duration)?;
        if frames == 0 {
            return Ok(());
        }
        let bytes = format.encode(&state.pcm[..frames * state.channels as usize]);
        let duration = frames as i64 * HNS_PER_SECOND / SAMPLE_RATE as i64;
        state.next_time = Some(time + duration);
        state.pending = Some(DecodedChunk {
            bytes,
            time,
            duration,
            discontinuity,
        });
        Ok(())
    }

    fn ProcessOutput(
        &self,
        _: u32,
        buffer_count: u32,
        buffers: *mut MFT_OUTPUT_DATA_BUFFER,
        status: *mut u32,
    ) -> Result<()> {
        if buffers.is_null() || status.is_null() {
            return Err(E_POINTER.into());
        }
        if buffer_count != 1 {
            return Err(E_INVALIDARG.into());
        }
        unsafe { *status = 0 };
        let sample = {
            let mut state = self.state.lock();
            let chunk = state
                .pending
                .as_ref()
                .ok_or(MF_E_TRANSFORM_NEED_MORE_INPUT)?;
            let sample = output_sample(chunk)?;
            state.pending = None;
            sample
        };
        let buffer = unsafe { &mut *buffers };
        buffer.dwStatus = 0;
        buffer.pSample = ManuallyDrop::new(Some(sample));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_channels_from_mp4_dops_box() {
        let sample_entry = hex(
            "000000374f707573000000000000000100000000000000000002001000000000bb80000000000013644f7073000200000000bb80000000",
        );
        assert_eq!(channels_from_codec_config(&sample_entry), Some(2));
    }

    #[test]
    fn reads_channels_from_opus_head() {
        let mut head = b"OpusHead".to_vec();
        head.extend_from_slice(&[1, 1, 0x38, 0x01, 0x80, 0xbb, 0, 0, 0, 0, 0]);
        assert_eq!(channels_from_codec_config(&head), Some(1));
    }

    #[test]
    fn pcm16_encoding_clamps_and_scales() {
        let bytes = SampleFormat::Pcm16.encode(&[1.5, -1.0, 0.0]);
        assert_eq!(bytes, [0xff, 0x7f, 0x01, 0x80, 0x00, 0x00]);
    }

    #[test]
    fn concealment_follows_the_sample_duration() {
        assert_eq!(concealment_frames(Some(200_000), 0), 960);
        assert_eq!(concealment_frames(Some(199_999), 0), 960);
        assert_eq!(concealment_frames(None, 480), 480);
        assert_eq!(concealment_frames(None, 0), 0);
        assert_eq!(
            concealment_frames(Some(10 * HNS_PER_SECOND), 0),
            MAX_FRAMES_PER_PACKET
        );
    }

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap_or(0))
            .collect()
    }
}
