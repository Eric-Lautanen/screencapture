//! `WASAPI` shapes: the four interfaces audio capture drives, pinned to
//! the slots the system headers declare, plus the wave-format records the
//! audio engine reads.
//!
//! The v-tables reuse the `interfaces!` macro the capture module declares
//! its `COM` layouts with, so slot order, layout pins, and the integrity
//! check behave identically here: only slots we call are typed, earlier
//! ones are placeholders that pin positions.

use std::ffi::c_void;

use crate::dxgi::abi::{Complete, Reserved, UnknownVt, interfaces};
use crate::dxgi::structs::Guid;
use crate::pin_layout;

// ---------------------------------------------------------------------------
// Interface declarations
// ---------------------------------------------------------------------------

interfaces! {
    (
        "`IMMDeviceEnumerator`: `EnumAudioEndpoints` 3, \
         `GetDefaultAudioEndpoint` 4, `GetDevice` 5.",
        EnumeratorVt: {
            enum_endpoints: EnumEndpoints,
            get_default_endpoint: DefaultEndpoint,
            get_device: GetDevice,
            register_endpoint_notification_callback: Reserved,
            unregister_endpoint_notification_callback: Reserved,
        },
    ),
    (
        "`IMMDevice`: `Activate` 3, `OpenPropertyStore` 4, `GetId` 5, \
         `GetState` 6.",
        DeviceVt: {
            activate: Activate,
            open_property_store: OpenStore,
            get_id: GetId,
            get_state: GetState,
        },
    ),
    (
        "`IMMDeviceCollection`: `GetCount` 3, `Item` 4.",
        CollectionVt: {
            get_count: GetCount,
            item: Item,
        },
    ),
    (
        "`IPropertyStore`: `GetValue` at 5.",
        PropertyStoreVt: {
            get_count: Reserved,
            get_at: Reserved,
            get_value: GetValue,
            set_value: Reserved,
            commit: Reserved,
        },
    ),
    (
        "`IAudioClient`: `Initialize` 3, `Start` 10, `Stop` 11, \
         `GetService` 14.",
        AudioClientVt: {
            initialize: Initialize,
            get_buffer_size: Reserved,
            get_stream_latency: Reserved,
            get_current_padding: Reserved,
            is_format_supported: Reserved,
            get_mix_format: Reserved,
            get_device_period: Reserved,
            start: Start,
            stop: Stop,
            reset: Reserved,
            set_event_handle: Reserved,
            get_service: GetService,
        },
    ),
    (
        "`IAudioCaptureClient`: `GetBuffer` 3, `ReleaseBuffer` 4, \
         `GetNextPacketSize` 5.",
        CaptureClientVt: {
            get_buffer: TakeBuffer,
            release_buffer: ReleaseBuffer,
            get_next_packet_size: NextPacket,
        },
    ),
}

// ---------------------------------------------------------------------------
// Slot signatures: every entry follows the Win32 `system` convention.
// ---------------------------------------------------------------------------

/// `GetDefaultAudioEndpoint(this, data_flow, role, device)`.
type DefaultEndpoint = unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> i32;
/// `EnumAudioEndpoints(this, data_flow, state_mask, collection)`.
type EnumEndpoints = unsafe extern "system" fn(*mut c_void, i32, u32, *mut *mut c_void) -> i32;
/// `GetDevice(this, id, device)`.
type GetDevice = unsafe extern "system" fn(*mut c_void, *const u16, *mut *mut c_void) -> i32;
/// `GetCount(this, count)` / `Item(this, index, device)` — the collection
/// and property store share the shape, differing in the out type.
type GetCount = unsafe extern "system" fn(*mut c_void, *mut u32) -> i32;
type Item = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
/// `OpenPropertyStore(this, access, store)`.
type OpenStore = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
/// `GetId(this, id)`: a `CoTaskMemAlloc`ed endpoint id.
type GetId = unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> i32;
/// `GetState(this, state)`.
type GetState = unsafe extern "system" fn(*mut c_void, *mut u32) -> i32;
/// `GetValue(this, key, value)`.
type GetValue = unsafe extern "system" fn(*mut c_void, *const PropertyKey, *mut PropVariant) -> i32;
/// `Activate(this, iid, class_context, activation_params, out)`.
type Activate =
    unsafe extern "system" fn(*mut c_void, *const Guid, u32, *mut c_void, *mut *mut c_void) -> i32;
/// `Initialize(this, share_mode, flags, buffer, period, format, session)`.
type Initialize =
    unsafe extern "system" fn(*mut c_void, i32, u32, i64, i64, *const c_void, *const Guid) -> i32;
/// `Start(this)` and `Stop(this)`.
type Start = unsafe extern "system" fn(*mut c_void) -> i32;
type Stop = unsafe extern "system" fn(*mut c_void) -> i32;
/// `GetService(this, iid, out)`.
type GetService = unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32;
/// `GetBuffer(this, data, frames, flags, device_position, qpc_position)`.
type TakeBuffer = unsafe extern "system" fn(
    *mut c_void,
    *mut *mut u8,
    *mut u32,
    *mut u32,
    *mut u64,
    *mut u64,
) -> i32;
/// `ReleaseBuffer(this, frames)`.
type ReleaseBuffer = unsafe extern "system" fn(*mut c_void, u32) -> i32;
/// `GetNextPacketSize(this, frames)`.
type NextPacket = unsafe extern "system" fn(*mut c_void, *mut u32) -> i32;

// ---------------------------------------------------------------------------
// Class and interface ids
// ---------------------------------------------------------------------------

/// `CLSID_MMDeviceEnumerator` (mmdeviceapi.h).
pub const DEVICE_ENUMERATOR: Guid = Guid::new(
    0xbcde_0395,
    0xe52f,
    0x467c,
    [0x8e, 0x3d, 0xc4, 0x57, 0x92, 0x91, 0x69, 0x2e],
);
/// `IID_IMMDeviceEnumerator`.
pub const ENUMERATOR: Guid = Guid::new(
    0xa956_64d2,
    0x9614,
    0x4f35,
    [0xa7, 0x46, 0xde, 0x8d, 0xb6, 0x36, 0x17, 0xe6],
);
/// `IID_IAudioClient`.
pub const AUDIO_CLIENT: Guid = Guid::new(
    0x1cb9_ad4c,
    0xdbfa,
    0x4c32,
    [0xb1, 0x78, 0xc2, 0xf5, 0x68, 0xa7, 0x03, 0xb2],
);
/// `IID_IAudioCaptureClient`.
pub const CAPTURE_CLIENT: Guid = Guid::new(
    0xc8ad_bd64,
    0xe71e,
    0x48a0,
    [0xa4, 0xde, 0x18, 0x5c, 0x39, 0x5c, 0xd3, 0x17],
);

/// `KSDATAFORMAT_SUBTYPE_PCM`, the sub-format that marks 16-bit samples.
pub const SUBTYPE_PCM: Guid = Guid::new(
    0x0000_0001,
    0x0000,
    0x0010,
    [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71],
);

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// `CLSCTX_ALL`: activate whatever context the class needs.
pub const CLSCTX_ALL: u32 = 0x17;
/// `eRender`: playback devices, the ones loopback records.
pub const RENDER: i32 = 0;
/// `eCapture`: recording devices, the microphone.
pub const CAPTURE: i32 = 1;
/// `eConsole`: the endpoint the user's console session uses.
pub const ROLE_CONSOLE: i32 = 0;
/// `eMultimedia`: the endpoint ordinary playback and recording apps use.
pub const ROLE_MULTIMEDIA: i32 = 1;
/// `AUDCLNT_SHAREMODE_SHARED`: mix with every other application.
pub const SHARED: i32 = 0;
/// `AUDCLNT_STREAMFLAGS_LOOPBACK`: capture what a render device plays.
pub const LOOPBACK: u32 = 0x0002_0000;
/// `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM`: let the engine convert between the
/// format we ask for and the endpoint's own mix format.
pub const AUTOCONVERT: u32 = 0x8000_0000;
/// `AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY`: higher-quality sample-rate
/// conversion, only meaningful together with [`AUTOCONVERT`].
pub const SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
/// `AUDCLNT_BUFFERFLAGS_SILENT`: the packet holds silence, not samples.
pub const SILENT: u32 = 0x2;
/// The stream is four seconds in the future in 100-nanosecond units.
pub const BUFFER_100MS: i64 = 1_000_000;
/// `AUDCLNT_E_DEVICE_INVALIDATED`: the endpoint disappeared mid-stream.
pub const DEVICE_INVALIDATED: i32 = -0x7776_FFFC;
/// `DEVICE_STATE_*` (mmdeviceapi.h): which endpoints an enumeration lists.
pub const DEVICE_STATE_ACTIVE: u32 = 0x0000_0001;
pub const DEVICE_STATE_DISABLED: u32 = 0x0000_0002;
pub const DEVICE_STATE_NOTPRESENT: u32 = 0x0000_0004;
pub const DEVICE_STATE_UNPLUGGED: u32 = 0x0000_0008;
/// Every endpoint the machine remembers, connected or not: a Bluetooth
/// headset that is switched off still has an id, which is what lets a
/// chosen microphone come back when it reconnects.
pub const DEVICE_STATEMASK_ALL: u32 =
    DEVICE_STATE_ACTIVE | DEVICE_STATE_DISABLED | DEVICE_STATE_NOTPRESENT | DEVICE_STATE_UNPLUGGED;
/// `STGM_READ`: the property store is read, never written.
pub const STGM_READ: u32 = 0;
/// `VT_LPWSTR`: a wide string, the type of a device's friendly name.
pub const VT_LPWSTR: u16 = 31;

/// `PROPERTYKEY` (wtypes.h): one named property of a device.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PropertyKey {
    /// The property set's GUID.
    pub set: Guid,
    /// The property's index inside that set.
    pub property: u32,
}

/// `PKEY_Device_FriendlyName`: the name Sound settings shows.
pub const DEVICE_NAME: PropertyKey = PropertyKey {
    set: Guid::new(
        0xa45c_254e,
        0xdf1c,
        0x4efd,
        [0x80, 0x20, 0x67, 0xd1, 0x46, 0xa8, 0x50, 0xe0],
    ),
    property: 14,
};

/// `PROPVARIANT` (propidl.h): a tagged value.
///
/// Only the tag and a string pointer are ever read here, so the union is
/// kept as a wide, eight-byte-aligned blob: it is the size of the real
/// record on both 32- and 64-bit Windows, and over-sized never loses a
/// byte `PropVariantClear` writes.
#[repr(C, align(8))]
pub struct PropVariant {
    /// `VARTYPE` of the value that follows.
    pub kind: u16,
    /// Reserved; must be zero.
    pub reserved: [u16; 3],
    /// The union, read only through [`Self::as_string`] after a tag check.
    data: [u8; 24],
}

impl PropVariant {
    /// An empty value, which is what `GetValue` fills in.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            kind: 0,
            reserved: [0; 3],
            data: [0; 24],
        }
    }

    /// The wide string this value holds, if it holds one.
    #[must_use]
    pub fn as_string(&self) -> Option<*const u16> {
        if self.kind != VT_LPWSTR {
            return None;
        }
        // SAFETY: the union starts after the eight-byte header and the
        // value is tagged `VT_LPWSTR`, so the first pointer is the string.
        let pointer = unsafe { self.data.as_ptr().cast::<*const u16>().read_unaligned() };
        if pointer.is_null() {
            return None;
        }
        Some(pointer)
    }
}

// ---------------------------------------------------------------------------
// Wave formats
// ---------------------------------------------------------------------------

/// `WAVEFORMATEX` (mmreg.h): the fixed header every wave format starts with.
///
/// mmreg.h wraps these records in `pshpack1`, so an 18-byte header sits at
/// the front of the extended form with no padding to a four-byte boundary.
/// Packing them here is what reproduces that layout exactly.
#[repr(C, packed(1))]
#[derive(Clone, Copy, Default)]
pub struct WaveFormat {
    /// `WAVE_FORMAT_*` tag; [`WAVE_FORMAT_EXTENSIBLE`] for the extended form.
    pub tag: u16,
    /// Channel count.
    pub channels: u16,
    /// Samples per second.
    pub sample_rate: u32,
    /// Bytes per second: rate times block alignment.
    pub avg_bytes: u32,
    /// Bytes per frame: channels times sample width.
    pub block_align: u16,
    /// Bits per sample, container width included.
    pub bits: u16,
    /// Bytes of extra descriptor that follow; 22 for the extended form.
    pub extra: u16,
}

// The audio engine reads this from our buffer out of process, so the layout
// is pinned exactly.
pin_layout!(WaveFormat, 18; tag = 0, channels = 2, sample_rate = 4,
    avg_bytes = 8, block_align = 12, bits = 14, extra = 16);

/// `WAVEFORMATEXTENSIBLE`: the header plus channel mask and sub-format,
/// which is how 16-bit stereo is described without ambiguity.
#[repr(C, packed(1))]
#[derive(Clone, Copy)]
pub struct WaveFormatExtensible {
    /// The shared header.
    pub base: WaveFormat,
    /// Bits actually used per sample; 16 for 16-bit PCM.
    pub valid_bits: u16,
    /// `SPEAKER_*` mask: 3 is front left plus front right.
    pub channel_mask: u32,
    /// [`SUBTYPE_PCM`] for integer samples.
    pub sub_format: Guid,
}

pin_layout!(WaveFormatExtensible, 40; base = 0, valid_bits = 18,
    channel_mask = 20, sub_format = 24);

/// `WAVE_FORMAT_EXTENSIBLE` (mmreg.h).
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// Channels this application asks for.
pub const CHANNELS: u16 = 2;
/// Sample rate this application asks for: what every encoder here takes.
pub const SAMPLE_RATE: u32 = 48_000;
/// Sample width this application asks for.
pub const BITS: u16 = 16;

/// 48 kHz stereo 16-bit PCM, the one format both capture clients are asked
/// for; the engine converts whatever the endpoints actually run at.
#[must_use]
pub fn stereo_pcm() -> WaveFormatExtensible {
    let block_align = CHANNELS * (BITS / 8);
    WaveFormatExtensible {
        base: WaveFormat {
            tag: WAVE_FORMAT_EXTENSIBLE,
            channels: CHANNELS,
            sample_rate: SAMPLE_RATE,
            avg_bytes: SAMPLE_RATE * u32::from(block_align),
            block_align,
            bits: BITS,
            extra: 22,
        },
        valid_bits: BITS,
        channel_mask: 0x3,
        sub_format: SUBTYPE_PCM,
    }
}
