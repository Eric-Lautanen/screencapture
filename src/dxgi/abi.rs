//! COM v-table shapes for the interfaces whose slots the capture pipeline
//! calls: hand-compiled from the mingw-w64 WIDL headers plus community
//! v-table dumps of the live system libraries. Only slots we actually call
//! are typed; earlier slots are opaque placeholders whose sole duty is
//! pinning positions, and every declared slot participates in the integrity
//! check that runs before the first call goes through.

use crate::dxgi::structs::{
    DuplDesc, FrameInfo, Guid, Mapped, OutputDesc, PointerShapeInfo, Tex2dDesc,
};
use crate::pin_layout;
use std::ffi::c_void;

// ---------------------------------------------------------------------------
// Slot types
// ---------------------------------------------------------------------------

/// `QueryInterface` slot.
pub type Query = unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32;
/// `AddRef` slot.
pub type Reference = unsafe extern "system" fn(*mut c_void) -> u32;
/// `Release` slot.
pub type Release = unsafe extern "system" fn(*mut c_void) -> u32;
/// Placeholder for reserved slots we never call; only position matters.
pub type Reserved = unsafe extern "system" fn();

/// The three `IUnknown` slots that open every COM v-table.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UnknownVt {
    /// `QueryInterface`.
    pub query: Query,
    /// `AddRef`.
    pub reference: Reference,
    /// `Release`.
    pub release: Release,
}

impl UnknownVt {
    /// True when all three slots hold resolved function pointers.
    pub(crate) fn intact(&self) -> bool {
        self.query as usize != 0 && self.reference as usize != 0 && self.release as usize != 0
    }
}

/// Type-level knowledge of an interface: its `IUnknown` prefix plus an
/// integrity check over every declared slot.
pub trait Complete {
    /// The v-table's leading `IUnknown` slots.
    fn unknown(&self) -> &UnknownVt;
    /// True when every declared slot holds a function pointer; callers
    /// prove their layout assumptions once, before any call goes out.
    /// (Reading slots is safe: our declared sets never exceed the real
    /// interface's, so nothing behind the allocation is touched.)
    fn complete(&self) -> bool;
}

// ---------------------------------------------------------------------------
// Interface declarations
// ---------------------------------------------------------------------------

macro_rules! interfaces {
    (
        $(
            ( $doc:literal, $name:ident : { $( $slot:ident : $ty:ty, )* }, ),
        )*
    ) => {
        $(
            #[doc = $doc]
            #[repr(C)]
            pub struct $name {
                /// `IUnknown` prefix shared by every COM v-table.
                pub unknown: UnknownVt,
                $( pub $slot: $ty, )*
            }

            impl Complete for $name {
                fn unknown(&self) -> &UnknownVt {
                    &self.unknown
                }

                fn complete(&self) -> bool {
                    self.unknown.intact() $( && self.$slot as usize != 0 )*
                }
            }

            // Layout pin: the `IUnknown` prefix must stay at slot 0, and
            // the whole table is one 8-byte fn pointer per slot after it
            // (every slot type here is a plain function pointer, so that
            // arithmetic is exact rather than approximate).
            const _: () = {
                assert!(
                    ::core::mem::offset_of!($name, unknown) == 0,
                    ::core::stringify!($name)
                );
                assert!(
                    ::core::mem::size_of::<$name>()
                        == 24 + 8usize * (0usize $( + { let _ = stringify!($slot); 1usize } )*),
                    ::core::stringify!($name)
                );
            };
        )*
    };
}

pin_layout!(UnknownVt, 24; query = 0, reference = 8, release = 16);

interfaces! {
    (
        "`IDXGIFactory1`: slots 0..=13; `EnumAdapters1` sits at 12.",
        FactoryVt: {
            set_private_data: Reserved,
            set_private_data_interface: Reserved,
            get_private_data: Reserved,
            get_parent: Reserved,
            enum_adapters: Reserved,
            make_window_association: Reserved,
            get_window_association: Reserved,
            create_swap_chain: Reserved,
            create_software_adapter: Reserved,
            enum_adapters1: Enumerate,
            is_current: Reserved,
        },
    ),
    (
        "`IDXGIAdapter`: slots 0..=9; `EnumOutputs` sits at 7.",
        AdapterVt: {
            set_private_data: Reserved,
            set_private_data_interface: Reserved,
            get_private_data: Reserved,
            get_parent: Reserved,
            enum_outputs: Enumerate,
            get_desc: Reserved,
            check_interface_support: Reserved,
        },
    ),
    (
        "`IDXGIOutput`: slots 0..=18; `GetDesc` sits at 7.",
        OutputVt: {
            set_private_data: Reserved,
            set_private_data_interface: Reserved,
            get_private_data: Reserved,
            get_parent: Reserved,
            get_desc: Describe,
            get_display_mode_list: Reserved,
            find_closest_matching_mode: Reserved,
            wait_for_v_blank: Reserved,
            take_ownership: Reserved,
            release_ownership: Reserved,
            get_gamma_control_capabilities: Reserved,
            set_gamma_control: Reserved,
            get_gamma_control: Reserved,
            set_display_surface: Reserved,
            get_display_surface_data: Reserved,
            get_frame_statistics: Reserved,
        },
    ),
    (
        "`IDXGIOutput1`: slots 0..=22; `DuplicateOutput` sits at 22.",
        Output1Vt: {
            set_private_data: Reserved,
            set_private_data_interface: Reserved,
            get_private_data: Reserved,
            get_parent: Reserved,
            get_desc: Describe,
            get_display_mode_list: Reserved,
            find_closest_matching_mode: Reserved,
            wait_for_v_blank: Reserved,
            take_ownership: Reserved,
            release_ownership: Reserved,
            get_gamma_control_capabilities: Reserved,
            set_gamma_control: Reserved,
            get_gamma_control: Reserved,
            set_display_surface: Reserved,
            get_display_surface_data: Reserved,
            get_frame_statistics: Reserved,
            get_display_mode_list1: Reserved,
            find_closest_matching_mode1: Reserved,
            get_display_surface_data1: Reserved,
            duplicate_output: Duplicate,
        },
    ),
    (
        "`IDXGIOutputDuplication`: slots 0..=14 with the full family.",
        DuplicationVt: {
            set_private_data: Reserved,
            set_private_data_interface: Reserved,
            get_private_data: Reserved,
            get_parent: Reserved,
            get_desc: DescribeOutputs,
            acquire_next_frame: Acquire,
            get_frame_dirty_rects: Reserved,
            get_frame_move_rects: Reserved,
            get_frame_pointer_shape: TakeShape,
            map_desktop_surface: Reserved,
            un_map_desktop_surface: Reserved,
            release_frame: ReleaseFrame,
        },
    ),
    (
        "`IDXGIResource`: slots 0..=2; held to copy, then freed.",
        ResourceVt: {},
    ),
    (
        "`ID3D11Device`: slots 0..=5; `CreateTexture2D` sits at 5.",
        DeviceVt: {
            create_buffer: Reserved,
            create_texture1d: Reserved,
            create_texture2d: StageTexture,
        },
    ),
    (
        "`ID3D11DeviceContext`: slots 0..=47; `Map` 14, `Unmap` 15, `CopyResource` 47.",
        ContextVt: {
            get_device: Reserved,
            get_private_data: Reserved,
            set_private_data: Reserved,
            set_private_data_interface: Reserved,
            vs_set_constant_buffers: Reserved,
            ps_set_shader_resources: Reserved,
            ps_set_shader: Reserved,
            ps_set_samplers: Reserved,
            vs_set_shader: Reserved,
            draw_indexed: Reserved,
            draw: Reserved,
            map: LockResource,
            unmap: UnlockResource,
            ps_set_constant_buffers: Reserved,
            ia_set_input_layout: Reserved,
            ia_set_vertex_buffers: Reserved,
            ia_set_index_buffer: Reserved,
            draw_indexed_instanced: Reserved,
            draw_instanced: Reserved,
            gs_set_constant_buffers: Reserved,
            gs_set_shader: Reserved,
            ia_set_primitive_topology: Reserved,
            vs_set_shader_resources: Reserved,
            vs_set_samplers: Reserved,
            begin: Reserved,
            end: Reserved,
            get_data: Reserved,
            set_predication: Reserved,
            gs_set_shader_resources: Reserved,
            gs_set_samplers: Reserved,
            om_set_render_targets: Reserved,
            om_set_render_targets_and_unordered_access_views: Reserved,
            om_set_blend_state: Reserved,
            om_set_depth_stencil_state: Reserved,
            so_set_targets: Reserved,
            draw_auto: Reserved,
            draw_indexed_instanced_indirect: Reserved,
            draw_instanced_indirect: Reserved,
            dispatch: Reserved,
            dispatch_indirect: Reserved,
            rs_set_state: Reserved,
            rs_set_viewports: Reserved,
            rs_set_scissor_rects: Reserved,
            copy_subresource_region: Reserved,
            copy_resource: CloneResource,
        },
    ),
    (
        "`ID3D11Texture2D`: slots 0..=2; held for the pipeline, then freed.",
        TextureVt: {},
    ),
}

// Signature aliases: every entry follows the Win32 `system` convention.
type Enumerate = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
type Describe = unsafe extern "system" fn(*mut c_void, *mut OutputDesc);
type DescribeOutputs = unsafe extern "system" fn(*mut c_void, *mut DuplDesc);
type Acquire = unsafe extern "system" fn(*mut c_void, u32, *mut FrameInfo, *mut *mut c_void) -> i32;
type ReleaseFrame = unsafe extern "system" fn(*mut c_void) -> i32;
type TakeShape = unsafe extern "system" fn(
    *mut c_void,
    u32,
    *mut c_void,
    *mut u32,
    *mut PointerShapeInfo,
) -> i32;
type Duplicate = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> i32;
type StageTexture = unsafe extern "system" fn(
    *mut c_void,
    *const Tex2dDesc,
    *const c_void,
    *mut *mut c_void,
) -> i32;
type LockResource =
    unsafe extern "system" fn(*mut c_void, *mut c_void, u32, u32, u32, *mut Mapped) -> i32;
type UnlockResource = unsafe extern "system" fn(*mut c_void, *mut c_void, u32);
type CloneResource = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void);
