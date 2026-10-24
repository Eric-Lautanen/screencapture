//! Owned COM handles plus the one-method-per-call protocol the capture
//! pipeline uses. This is the only place in the module family that reads
//! or writes interface memory; `mod.rs` sees the protocol surface only.

use std::ffi::c_void;

use crate::dxgi::abi::{
    AdapterVt, Complete, ContextVt, DeviceVt, DuplicationVt, FactoryVt, Output1Vt, OutputVt,
    ResourceVt, TextureVt,
};
use crate::dxgi::structs::{
    B8G8R8A8, CPU_READ, DuplDesc, FrameInfo, MAP_READ, Mapped, OutputDesc, S_OK, STAGING,
    SampleDesc, Tex2dDesc, WAIT_TIMEOUT,
};

/// Error code used when interface memory stopped presenting a v-table; the
/// binary's own data cannot reach this without heap corruption.
const ADRIFT: i32 = -1;

/// One owned COM interface: still a reference count, released on drop.
pub struct Com<T: Complete> {
    /// Points at the interface whose first word is its v-table.
    connection: *mut *mut T,
}

impl<T: Complete> Com<T> {
    /// An empty handle: holding nothing, dropping releases nothing. Used to
    /// park a stale session before its replacement is created, because the
    /// driver refuses a second duplication while the first one lives.
    /// Calls through it answer `ADRIFT`.
    #[must_use]
    pub fn vacant() -> Self {
        Self {
            connection: std::ptr::null_mut(),
        }
    }

    /// Adopt a freshly created interface whose creation already rejected
    /// null answers.
    ///
    /// # Safety
    /// `raw` must be a live interface pointer carrying the one reference
    /// the creation call handed us. A dangling pointer here would read a
    /// foreign v-table and release it on drop, which is why adoption is an
    /// `unsafe` step rather than a plain constructor: the raw pointer alone
    /// cannot prove the reference exists.
    pub(crate) unsafe fn own(raw: *mut c_void) -> Self {
        Self {
            connection: raw.cast::<*mut T>(),
        }
    }

    /// Cast this interface to another interface named by `guid`.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT` when the other side refuses the cast.
    pub fn query_to<U: Complete>(&self, guid: &crate::dxgi::structs::Guid) -> Result<Com<U>, i32> {
        let mut cast: *mut c_void = std::ptr::null_mut();
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let query = table.unknown().query;
        // SAFETY: both sides here are live COM objects; the GUID layout
        // matches REFIID and the out slot is a local.
        let hr = unsafe { query(self.connection.cast::<c_void>(), guid, &raw mut cast) };
        if hr != S_OK || cast.is_null() {
            return Err(hr);
        }
        // SAFETY: `QueryInterface` returned S_OK with a non-null interface
        // that holds the reference it just added for us.
        Ok(unsafe { Com::own(cast) })
    }

    /// The raw object pointer, for entry points typed `void *`.
    #[must_use]
    pub fn raw(&self) -> *mut c_void {
        self.connection.cast::<c_void>()
    }

    /// The interface memory when it still presents a v-table.
    pub(crate) fn view(&self) -> Option<&T> {
        if self.connection.is_null() {
            return None;
        }
        // SAFETY: `connection` is a live interface pointer whose first word
        // is the v-table; reading it touches no state of its own.
        unsafe { (*self.connection).as_ref() }
    }

    /// Whether the interface presents a complete, well-shaped v-table.
    #[must_use]
    pub fn intact(&self) -> bool {
        self.view().is_some_and(Complete::complete)
    }
}

impl<T: Complete> Drop for Com<T> {
    fn drop(&mut self) {
        if let Some(table) = self.view() {
            let release = table.unknown().release;
            // SAFETY: the object outlives this body; slot 2 takes just the
            // object pointer.
            unsafe { release(self.connection.cast::<c_void>()) };
        }
        self.connection = std::ptr::null_mut();
    }
}

// ---------------------------------------------------------------------------
// Protocol: one method per COM call the capture pipeline makes
// ---------------------------------------------------------------------------

/// A fresh zeroed `DXGI_OUTDUPL_FRAME_INFO`.
fn zeroed_info() -> crate::dxgi::structs::FrameInfo {
    // SAFETY: every field is valid at byte zero (a null pointer state).
    unsafe { std::mem::zeroed::<crate::dxgi::structs::FrameInfo>() }
}

/// A fresh zeroed `DXGI_OUTDUPL_DESC`.
fn zeroed_outputs() -> DuplDesc {
    // SAFETY: every field is valid at byte zero (a null pointer state).
    unsafe { std::mem::zeroed::<DuplDesc>() }
}

impl Com<FactoryVt> {
    /// `EnumAdapters1`: grab the adapter that owns the desktop outputs.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT` outside success.
    pub fn adapter1(&self, index: u32) -> Result<Com<AdapterVt>, i32> {
        let mut out: *mut c_void = std::ptr::null_mut();
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let enumerate = table.enum_adapters1;
        // SAFETY: live interface and out slot; slot 12 takes exactly this.
        let hr = unsafe { enumerate(self.connection.cast::<c_void>(), index, &raw mut out) };
        if hr != S_OK || out.is_null() {
            return Err(hr);
        }
        // SAFETY: `EnumAdapters1` returned S_OK with a non-null adapter
        // whose reference is now ours.
        Ok(unsafe { Com::own(out) })
    }
}

impl Com<AdapterVt> {
    /// `EnumOutputs`: the nth output attached to the adapter, ordered by
    /// the system's display arrangement.
    ///
    /// # Errors
    /// Fails with `NOT_FOUND` past the last output, else raw `HRESULT`.
    pub fn output(&self, index: u32) -> Result<Com<OutputVt>, i32> {
        let mut out: *mut c_void = std::ptr::null_mut();
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let enumerate = table.enum_outputs;
        // SAFETY: live interface and out slot; slot 7 takes exactly this.
        let hr = unsafe { enumerate(self.connection.cast::<c_void>(), index, &raw mut out) };
        if hr != S_OK || out.is_null() {
            return Err(hr);
        }
        // SAFETY: `EnumOutputs` returned S_OK with a non-null output whose
        // reference is now ours.
        Ok(unsafe { Com::own(out) })
    }
}

impl Com<OutputVt> {
    /// `GetDesc`: display-location facts for output selection.
    ///
    /// The call itself has no result code; a zeroed answer reads as "not a
    /// match" upstream, which is all selection needs.
    pub fn describe(&self) -> OutputDesc {
        let mut description = zeroed_output();
        let Some(table) = self.view() else {
            return description;
        };
        let describe = table.get_desc;
        // SAFETY: live interface; `description` is a full `OutputDesc`.
        unsafe { describe(self.connection.cast::<c_void>(), &raw mut description) };
        description
    }
}

fn zeroed_output() -> OutputDesc {
    // SAFETY: every field is valid at byte zero (integers, a NUL test, and
    // a null handle).
    unsafe { std::mem::zeroed::<OutputDesc>() }
}

impl Com<Output1Vt> {
    /// `DuplicateOutput`: begin mirroring this output with `device`.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT` (busy, low memory, unsupported).
    pub fn duplication(&self, device: &Com<DeviceVt>) -> Result<Com<DuplicationVt>, i32> {
        let mut out: *mut c_void = std::ptr::null_mut();
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let duplicate = table.duplicate_output;
        // SAFETY: live interfaces on both sides and a local out slot; slot
        // 22 takes exactly these three arguments.
        let hr = unsafe { duplicate(self.connection.cast::<c_void>(), device.raw(), &raw mut out) };
        if hr != S_OK || out.is_null() {
            return Err(hr);
        }
        // SAFETY: `DuplicateOutput` returned S_OK with a non-null session
        // whose reference is now ours.
        Ok(unsafe { Com::own(out) })
    }
}

impl Com<DuplicationVt> {
    /// `GetDesc`: the mode duplication produces (shape and pixel format).
    pub fn descriptor(&self) -> DuplDesc {
        let mut state = zeroed_outputs();
        let Some(table) = self.view() else {
            return state;
        };
        let describe = table.get_desc;
        // SAFETY: live interface; `state` is a full `DuplDesc`.
        unsafe { describe(self.connection.cast::<c_void>(), &raw mut state) };
        state
    }

    /// `AcquireNextFrame`: wait up to `wait` ms for a desktop update.
    ///
    /// Returns the acquired desktop resource (none on timeout) alongside
    /// the frame metadata, which carries pointer position and the pending
    /// pointer-shape budget.
    ///
    /// # Errors
    /// Fails with raw `HRESULT` for session-breaking conditions; a `None`
    /// resource means the wait expired with nothing new.
    pub fn acquire(&self, wait: u32) -> Result<(Option<Com<ResourceVt>>, FrameInfo), i32> {
        let mut resources: *mut c_void = std::ptr::null_mut();
        let mut info = zeroed_info();
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let acquire = table.acquire_next_frame;
        // SAFETY: live interface and local out params; slot 8 takes
        // exactly this (the frame info is write-only for us).
        let hr = unsafe {
            acquire(
                self.connection.cast::<c_void>(),
                wait,
                &raw mut info,
                &raw mut resources,
            )
        };
        if hr == WAIT_TIMEOUT {
            return Ok((None, info));
        }
        if hr != S_OK || resources.is_null() {
            return Err(hr);
        }
        // SAFETY: `AcquireNextFrame` returned S_OK with a non-null resource
        // that holds its reference until `ReleaseFrame`.
        Ok((Some(unsafe { Com::own(resources) }), info))
    }

    /// `GetFramePointerShape` with an empty buffer: the size-probe leg of
    /// the protocol. When the driver holds a shape to hand over it answers
    /// with the required byte count, which the real fetch then allocates.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT` when no shape is offered (including
    /// the trivial "nothing pending" case).
    pub fn pointer_shape_size(&self) -> Result<u32, i32> {
        let mut info = crate::dxgi::structs::PointerShapeInfo::default();
        let mut required: u32 = 0;
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let fetch = table.get_frame_pointer_shape;
        // SAFETY: live interface; the empty buffer is the documented probe.
        let hr = unsafe {
            fetch(
                self.connection.cast::<c_void>(),
                0,
                std::ptr::null_mut(),
                &raw mut required,
                &raw mut info,
            )
        };
        if required == 0 {
            return Err(hr);
        }
        Ok(required)
    }

    /// `ReleaseFrame`: hand the acquired desktop transaction back.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT` when the session was already lost.
    pub fn release_frame(&self) -> Result<(), i32> {
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let release = table.release_frame;
        // SAFETY: live interface; slot 14 takes exactly this.
        let hr = unsafe { release(self.connection.cast::<c_void>()) };
        (hr == S_OK).then_some(()).ok_or(hr)
    }

    /// `GetFramePointerShape`: copy the pending cursor bitmap into `shape`.
    /// The driver only owes bytes while `AcquireNextFrame` reported a
    /// nonzero `PointerShapeBufferSize`.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT`; `ACCESS_LOST` is routine here and the
    /// caller just keeps the previous shape.
    pub fn pointer_shape(
        &self,
        shape: &mut [u8],
    ) -> Result<crate::dxgi::structs::PointerShapeInfo, i32> {
        let mut info = crate::dxgi::structs::PointerShapeInfo::default();
        let mut required: u32 = 0;
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let fetch = table.get_frame_pointer_shape;
        let budget: u32 = shape.len().try_into().map_err(|_| ADRIFT)?;
        // SAFETY: live interface; the buffer is ours with the promised
        // length, and slot 11 takes exactly these five arguments.
        let hr = unsafe {
            fetch(
                self.connection.cast::<c_void>(),
                budget,
                shape.as_mut_ptr().cast::<c_void>(),
                &raw mut required,
                &raw mut info,
            )
        };
        if hr != S_OK {
            return Err(hr);
        }
        Ok(info)
    }
}

impl Com<DeviceVt> {
    /// `CreateTexture2D`: a CPU-readable staging twin of a frame shape.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT` from the driver.
    pub fn staged(&self, width: u32, height: u32) -> Result<Com<TextureVt>, i32> {
        let mut out: *mut c_void = std::ptr::null_mut();
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let description = Tex2dDesc {
            width,
            height,
            mips: 1,
            array: 1,
            format: B8G8R8A8,
            sample: SampleDesc {
                count: 1,
                quality: 0,
            },
            usage: STAGING,
            bind: 0,
            cpu: CPU_READ,
            misc: 0,
        };
        let stage = table.create_texture2d;
        // SAFETY: live interface; the description and out slot are locals
        // and slot 5 takes exactly this (no initial contents).
        let hr = unsafe {
            stage(
                self.connection.cast::<c_void>(),
                &raw const description,
                std::ptr::null(),
                &raw mut out,
            )
        };
        if hr != S_OK || out.is_null() {
            return Err(hr);
        }
        // SAFETY: `CreateTexture2D` returned S_OK with a non-null texture
        // whose reference is now ours.
        Ok(unsafe { Com::own(out) })
    }
}

impl Com<ContextVt> {
    /// `CopyResource`: blast the whole texture into the staging twin.
    pub fn clone_resource(&self, destination: &Com<TextureVt>, source: &Com<TextureVt>) {
        let Some(table) = self.view() else {
            return;
        };
        let clone = table.copy_resource;
        // SAFETY: live interfaces and same-sized textures; slot 47 takes
        // exactly this and never fails asynchronously on our path.
        unsafe {
            clone(
                self.connection.cast::<c_void>(),
                destination.raw(),
                source.raw(),
            );
        };
    }

    /// `Map`: lock the staging texture for CPU reads. The answer unmaps
    /// itself on drop, so every call site cannot help but pair `Map` with
    /// `Unmap`, even when the copy in between panics.
    ///
    /// # Errors
    /// Fails with the raw `HRESULT` when the GPU queue is in trouble.
    pub fn lock<'a>(&'a self, texture: &'a Com<TextureVt>) -> Result<Locked<'a>, i32> {
        let mut mapped: Mapped = Mapped::default();
        let Some(table) = self.view() else {
            return Err(ADRIFT);
        };
        let lock = table.map;
        // SAFETY: live interfaces; slot 14 takes exactly this.
        let hr = unsafe {
            lock(
                self.connection.cast::<c_void>(),
                texture.raw(),
                0, // subresource index stays zero
                MAP_READ,
                0, // no map flags
                &raw mut mapped,
            )
        };
        if hr != S_OK {
            return Err(hr);
        }
        Ok(Locked {
            context: self,
            texture,
            mapped,
        })
    }

    /// `Unmap`: hand the staging texture back to the GPU. Only [`Locked`]
    /// calls this, which is what keeps every mapping paired.
    fn unlock(&self, texture: &Com<TextureVt>) {
        let Some(table) = self.view() else {
            return;
        };
        let unlock = table.unmap;
        // SAFETY: live interfaces; slot 15 takes exactly this.
        unsafe {
            unlock(self.connection.cast::<c_void>(), texture.raw(), 0);
        };
    }
}

/// A staging texture locked for CPU reads; dropping hands it back.
///
/// The guard borrows the context and the texture, so the mapped bytes
/// cannot outlive the lock, and it is neither `Copy` nor `Clone`, so one
/// mapping is unmapped exactly once. Before this guard, each call site had
/// to remember the `unlock` after the read, and a panic in between left the
/// texture mapped for the rest of the recording.
pub struct Locked<'a> {
    context: &'a Com<ContextVt>,
    texture: &'a Com<TextureVt>,
    mapped: Mapped,
}

impl Locked<'_> {
    /// Bytes per mapped row: the driver's pitch, not the frame's width.
    #[must_use]
    pub fn row_pitch(&self) -> usize {
        usize::try_from(self.mapped.row_pitch).unwrap_or(0)
    }

    /// The mapped pixels: `rows` rows of [`Self::row_pitch`] bytes each.
    ///
    /// # Safety
    /// `rows` must not exceed the locked texture's height. The driver
    /// mapped exactly that many rows, so claiming more would form a slice
    /// past the mapping. The guard keeps the mapping alive for the borrow.
    #[must_use]
    pub unsafe fn bytes(&self, rows: usize) -> &[u8] {
        if self.mapped.data.is_null() {
            return &[];
        }
        // SAFETY: the caller bounds `rows` by the texture height and the
        // guard still holds the lock, so `rows * pitch` bytes are mapped
        // and readable until this borrow ends.
        unsafe {
            std::slice::from_raw_parts(
                self.mapped.data.cast::<u8>(),
                self.row_pitch().saturating_mul(rows),
            )
        }
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        self.context.unlock(self.texture);
    }
}
