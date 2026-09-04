use std::{
    ffi::CStr,
    marker::PhantomData,
    rc::Rc,
};

use ash::vk::Handle;
use uuid::Uuid;

use super::ffi;
use crate::libplacebo::{
    Gpu,
    Log,
    log,
};

#[derive(Debug, PartialEq, Eq)]
pub enum VulkanDeviceSelector<'a> {
    Default { allow_software: bool },
    Name(&'a str),
    Uuid(Uuid),
}

impl Default for VulkanDeviceSelector<'_> {
    fn default() -> Self {
        Self::Default {
            allow_software: false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum VulkanDeviceSelectorExt<'a> {
    Default { allow_software: bool },
    Existing(ash::vk::PhysicalDevice),
    Name(&'a str),
    Uuid(Uuid),
}

impl Default for VulkanDeviceSelectorExt<'_> {
    fn default() -> Self {
        Self::Default {
            allow_software: false,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum VulkanComputeMode {
    /// Disables use of Compute Shaders
    Disabled,
    /// Disables Asynchronous Compute
    Sync,
    /// Enables Asynchronous Compute
    #[default]
    Async,
}

pub enum VulkanSource<'a> {
    CreateNew {
        device: VulkanDeviceSelector<'a>,

        /// If set, enable the debugging and validation layers. These should
        /// generally be lightweight and relatively harmless to enable.
        debug:       bool,
        /// If set, also enable GPU-assisted verification and best practices
        /// layers. (Note: May cause substantial slowdown and/or result in lots
        /// of false positive spam)
        debug_extra: bool,

        /// Enables extra instance extensions. Instance creation will fail if
        /// these extensions are not all supported. The user may use
        /// this to enable e.g. windowing system integration.
        required_extensions: &'a [&'a str],
        /// Enables extra optional instance extensions. These are
        /// opportunistically enabled if supported by the device, but
        /// otherwise skipped.
        optional_extensions: &'a [&'a str],

        /// Enables extra layers. Instance creation will fail if these layers
        /// are not all supported.
        required_layers: &'a [&'a str],
        /// Enables extra optional layers. These are opportunistically enabled
        /// if supported by the platform, but otherwise skipped.
        optional_layers: &'a [&'a str],
    },
    Existing {
        instance: ash::vk::Instance,
        device:   VulkanDeviceSelectorExt<'a>,
    },
}

impl Default for VulkanSource<'_> {
    fn default() -> Self {
        Self::CreateNew {
            device:              VulkanDeviceSelector::default(),
            debug:               false,
            debug_extra:         false,
            required_extensions: &[],
            optional_extensions: &[],
            required_layers:     &[],
            optional_layers:     &[],
        }
    }
}

pub struct InitParamsCreate<'a> {
    pub entry:   ash::Entry,
    pub vulkan:  VulkanSource<'a>,
    pub surface: Option<ash::vk::SurfaceKHR>,

    /// Controls whether to use Compute Shaders, and if they should be queued
    /// asynchronously.
    pub compute_mode:   VulkanComputeMode,
    /// Controls whether or not to allow asynchronous transfers
    pub async_transfer: bool,

    /// Limits the number of queues to use. If `None`, libplacebo will use as
    /// many queues as the device supports.
    pub max_queues:   Option<usize>,
    /// Bitmask of extra queue families to enable. If set, then *all* queue
    /// families matching *any* of these flags will be enabled at device
    /// creation time.
    pub extra_queues: Option<ash::vk::QueueFlags>,

    /// Enables extra device extensions. Device creation will fail if these
    /// extensions are not all supported.
    pub extensions:     &'a [&'a str],
    /// Enables extra optional device extensions. These are opportunistically
    /// enabled if supported by the device, but otherwise skipped.
    pub opt_extensions: &'a [&'a str],
    /// Optional extra features to enable at device creation time. These are
    /// opportunistically enabled if supported by the device, but otherwise
    /// skipped.
    pub features:       Option<ash::vk::PhysicalDeviceFeatures2<'a>>,

    pub max_glsl_version:   i32,
    pub max_vk_api_version: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct VkQueueFamily {
    /// Queue Family Index
    pub index: u32,
    /// No. of Queues created at Device Initalization
    pub count: u32,
}

impl VkQueueFamily {
    pub const fn as_raw(self) -> ffi::pl_vulkan_queue {
        ffi::pl_vulkan_queue {
            index: self.index,
            count: self.count,
        }
    }
}

pub struct InitParamsExisting<'a> {
    pub entry:    ash::Entry,
    pub instance: ash::vk::Instance,

    pub phys_device: ash::vk::PhysicalDevice,
    pub device:      ash::vk::Device,

    /// List of all device-level extensions that were enabled.
    pub device_extensions: &'a [&'a CStr],
    /// Enabled [`PhysicalDeviceFeatures`](ash::vk::PhysicalDeviceFeatures). The
    /// device *must* be created with all of the features in
    /// [`GpuVk::required_device_features`] enabled.
    pub device_features:   ash::vk::PhysicalDeviceFeatures2<'a>,

    /// Must support [`QueueFlags::GRAPHICS`](ash::vk::QueueFlags::GRAPHICS)
    pub graphics_queue: VkQueueFamily,
    /// Must support  [`QueueFlags::COMPUTE`](ash::vk::QueueFlags::COMPUTE)
    pub compute_queue:  Option<VkQueueFamily>,
    /// Must support  [`QueueFlags::TRANSFER`](ash::vk::QueueFlags::TRANSFER)
    pub transfer_queue: Option<VkQueueFamily>,

    /// Disable use of Compute Shaders
    pub disable_compute: bool,

    pub max_glsl_version:   i32,
    pub max_vk_api_version: u32,
    // TODO: Support custom queue locking functions (too lazy to implement rn)
}

// Can be constructed 3 different ways; Entirely by libplacebo, with a supplied
// VkInstance (and optionally VkPhysicalDevice), and with a supplied
// VkDevice + VkQueue
pub struct GpuVk<'a> {
    handle:  ffi::pl_vulkan,
    _log:    Log,
    _marker: PhantomData<&'a ()>,
}

impl Drop for GpuVk<'_> {
    fn drop(&mut self) {
        unsafe {
            ffi::pl_vulkan_destroy(&raw mut self.handle);
        }
    }
}

impl<'a> GpuVk<'a> {
    /// For purely informative reasons, this contains a list of extensions that
    /// libplacebo *can* make use of. These are all strictly optional, but
    /// provide a hint to the API user as to what might be worth enabling at
    /// device creation time.
    pub fn recommended_device_extensions() -> impl Iterator<Item = &'static CStr> {
        unsafe {
            let extensions_ptr = ffi::pl_vulkan_recommended_extensions.as_ptr();
            let num_extensions = ffi::pl_vulkan_num_recommended_extensions;

            let extensions_raw =
                std::slice::from_raw_parts(extensions_ptr, num_extensions.cast_unsigned() as usize);
            extensions_raw.iter().map(|ptr| CStr::from_ptr(*ptr))
        }
    }

    /// For purely informative reasons, this contains a list of device features
    /// that libplacebo *can* make use of. These are all strictly optional,
    /// but provide a hint to the API user as to what might be worth enabling at
    /// device creation time.
    ///
    /// Note: This also includes physical device features provided by
    /// extensions. They are all provided using extension-specific features
    /// structs, rather than the more general purpose
    /// [`PhysicalDeviceVulkan11Features`](ash::vk::PhysicalDeviceVulkan11Features) etc.
    pub fn recommended_device_features() -> ash::vk::PhysicalDeviceFeatures2<'static> {
        unsafe {
            let features_raw = ffi::pl_vulkan_recommended_features;
            ash::vk::PhysicalDeviceFeatures2 {
                p_next: features_raw.pNext,
                features: std::mem::transmute::<
                    ffi::VkPhysicalDeviceFeatures,
                    ash::vk::PhysicalDeviceFeatures,
                >(features_raw.features),
                ..ash::vk::PhysicalDeviceFeatures2::default()
            }
        }
    }

    /// A list of device features that are required by libplacebo. These
    /// *must* be provided by imported Vulkan devices.
    ///
    /// Note: [`GpuVk::recommended_device_features`] does not include this list.
    pub fn required_device_features() -> ash::vk::PhysicalDeviceFeatures2<'static> {
        unsafe {
            let features_raw = ffi::pl_vulkan_required_features;
            ash::vk::PhysicalDeviceFeatures2 {
                p_next: features_raw.pNext,
                features: std::mem::transmute::<
                    ffi::VkPhysicalDeviceFeatures,
                    ash::vk::PhysicalDeviceFeatures,
                >(features_raw.features),
                ..ash::vk::PhysicalDeviceFeatures2::default()
            }
        }
    }

    pub fn new(
        log: Log,
        params: &InitParamsCreate,
    ) -> Self {
        todo!()
    }

    /// Libplacebo only returns a nullptr on failure and outputs errors to Log,
    /// so we don't actually have any info to put into a Result
    pub fn import(
        log: Log,
        params: &InitParamsExisting,
    ) -> Option<Self> {
        unsafe {
            let extensions = params
                .device_extensions
                .iter()
                .map(|ext| ext.as_ptr().cast())
                .collect::<Box<_>>();

            #[allow(clippy::missing_transmute_annotations)]
            let raw_params = ffi::pl_vulkan_import_params {
                instance:       std::mem::transmute::<ash::vk::Instance, ffi::VkInstance>(
                    params.instance,
                ),
                get_proc_addr:  Some(std::mem::transmute(
                    params.entry.static_fn().get_instance_proc_addr,
                )),
                phys_device:    std::mem::transmute::<ash::vk::PhysicalDevice, ffi::VkPhysicalDevice>(
                    params.phys_device,
                ),
                device:         std::mem::transmute::<ash::vk::Device, ffi::VkDevice>(
                    params.device,
                ),
                extensions:     extensions.as_ptr(),
                num_extensions: extensions.len() as i32,

                queue_graphics: params.graphics_queue.as_raw(),
                queue_compute:  params.compute_queue.map_or(
                    ffi::pl_vulkan_queue { index: 0, count: 0 },
                    VkQueueFamily::as_raw,
                ),
                queue_transfer: params.transfer_queue.map_or(
                    ffi::pl_vulkan_queue { index: 0, count: 0 },
                    VkQueueFamily::as_raw,
                ),

                features: (&raw const params.device_features).cast(),

                lock_queue:   None,
                unlock_queue: None,
                queue_ctx:    std::ptr::null_mut(),

                no_compute:       params.disable_compute,
                max_glsl_version: params.max_glsl_version,
                max_api_version:  params.max_vk_api_version,
            };

            let inst = ffi::pl_vulkan_import(log.handle(), &raw const raw_params);
            if inst.is_null() {
                None
            } else {
                Some(Self {
                    handle:  inst,
                    _log:    log,
                    _marker: PhantomData,
                })
            }
        }
    }

    pub const unsafe fn handle(&self) -> ffi::pl_vulkan {
        self.handle
    }

    pub const fn as_gpu(&self) -> Gpu<'a> {
        unsafe {
            Gpu {
                handle:  self.handle.read().gpu,
                _marker: PhantomData,
            }
        }
    }
}
