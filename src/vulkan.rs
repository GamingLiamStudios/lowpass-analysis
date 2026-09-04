use std::{
    ffi::CStr,
    marker::PhantomData,
    rc::Rc,
};

use ash::vk::TaggedStructure;
use ffmpeg_next as ffmpeg;
use tracing::warn;
use wgpu_hal::Instance;

use super::{
    HWDeviceError,
    WgpuInitError,
    libplacebo,
};

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct PhysicalDeviceVulkan14Features<'a> {
    pub s_type:                                      ash::vk::StructureType,
    pub p_next:                                      *mut std::ffi::c_void,
    pub global_priority_query:                       ash::vk::Bool32,
    pub shader_subgroup_rotate:                      ash::vk::Bool32,
    pub shader_subgroup_rotate_clustered:            ash::vk::Bool32,
    pub shader_float_controls2:                      ash::vk::Bool32,
    pub shader_expect_assume:                        ash::vk::Bool32,
    pub rectangular_lines:                           ash::vk::Bool32,
    pub bresenham_lines:                             ash::vk::Bool32,
    pub smooth_lines:                                ash::vk::Bool32,
    pub stippled_rectangular_lines:                  ash::vk::Bool32,
    pub stippled_bresenham_lines:                    ash::vk::Bool32,
    pub stippled_smooth_lines:                       ash::vk::Bool32,
    pub vertex_attribute_instance_rate_divisor:      ash::vk::Bool32,
    pub vertex_attribute_instance_rate_zero_divisor: ash::vk::Bool32,
    pub index_type_uint8:                            ash::vk::Bool32,
    pub dynamic_rendering_local_read:                ash::vk::Bool32,
    pub maintenance5:                                ash::vk::Bool32,
    pub maintenance6:                                ash::vk::Bool32,
    pub pipeline_protected_access:                   ash::vk::Bool32,
    pub pipeline_robustness:                         ash::vk::Bool32,
    pub host_image_copy:                             ash::vk::Bool32,
    pub push_descriptor:                             ash::vk::Bool32,
    pub _marker:                                     PhantomData<&'a ()>,
}

impl PhysicalDeviceVulkan14Features<'_> {
    pub const STRUCTURE_TYPE: ash::vk::StructureType = ash::vk::StructureType::from_raw(55);
}

impl Default for PhysicalDeviceVulkan14Features<'_> {
    fn default() -> Self {
        Self {
            s_type:  Self::STRUCTURE_TYPE,
            p_next:  std::ptr::null_mut(),
            _marker: PhantomData,

            shader_subgroup_rotate:                      ash::vk::Bool32::default(),
            global_priority_query:                       ash::vk::Bool32::default(),
            shader_subgroup_rotate_clustered:            ash::vk::Bool32::default(),
            shader_float_controls2:                      ash::vk::Bool32::default(),
            shader_expect_assume:                        ash::vk::Bool32::default(),
            rectangular_lines:                           ash::vk::Bool32::default(),
            bresenham_lines:                             ash::vk::Bool32::default(),
            smooth_lines:                                ash::vk::Bool32::default(),
            stippled_rectangular_lines:                  ash::vk::Bool32::default(),
            stippled_bresenham_lines:                    ash::vk::Bool32::default(),
            stippled_smooth_lines:                       ash::vk::Bool32::default(),
            vertex_attribute_instance_rate_divisor:      ash::vk::Bool32::default(),
            vertex_attribute_instance_rate_zero_divisor: ash::vk::Bool32::default(),
            index_type_uint8:                            ash::vk::Bool32::default(),
            dynamic_rendering_local_read:                ash::vk::Bool32::default(),
            maintenance5:                                ash::vk::Bool32::default(),
            maintenance6:                                ash::vk::Bool32::default(),
            pipeline_protected_access:                   ash::vk::Bool32::default(),
            pipeline_robustness:                         ash::vk::Bool32::default(),
            host_image_copy:                             ash::vk::Bool32::default(),
            push_descriptor:                             ash::vk::Bool32::default(),
        }
    }
}

macro_rules! features2_operations {
    ($($features:ty[$($entry:ident),*]),*) => {
        unsafe fn destroy_features2(features: &mut ash::vk::PhysicalDeviceFeatures2) {
            unsafe {
                let mut p_next = features.p_next.cast::<ash::vk::BaseInStructure>();
                while !p_next.is_null() {
                    let feature = p_next.read();
                    match feature.s_type {
                        $(
                            <$features>::STRUCTURE_TYPE => {
                                let feature_ptr = p_next.cast::<$features>();
                                _ = Box::from_raw(feature_ptr);
                            }
                        ),*
                        ty => panic!(
                            "destroy_features2 was supplied an invalid/unsupported PhysicalDeviceFeatures2 p_next chain ({ty:?})",
                        ),
                    }
                    p_next = feature.p_next.cast_mut();
                }
            }
        }

        fn merge_features2(
            target: &mut ash::vk::PhysicalDeviceFeatures2,
            other: &ash::vk::PhysicalDeviceFeatures2,
        ) {
            unsafe {
                let mut other_types = Vec::new();

                let mut p_next = other.p_next.cast::<ash::vk::BaseInStructure>();
                while !p_next.is_null() {
                    let feature = p_next.read();
                    other_types.push((feature.s_type, p_next));
                    p_next = feature.p_next.cast_mut();
                }
                other_types.sort_unstable_by_key(|(ty, _)| *ty);

                let mut p_next = target.p_next.cast::<ash::vk::BaseInStructure>();
                while !p_next.is_null() {
                    let feature = p_next.read();
                    let Ok(other_idx) =
                        other_types.binary_search_by_key(&feature.s_type, |(ty, _)| *ty)
                    else {
                        p_next = feature.p_next.cast_mut();
                        continue;
                    };
                    let (_, other) = other_types.remove(other_idx);
                    match feature.s_type {
                        $(
                            <$features>::STRUCTURE_TYPE => {
                                let feature = p_next.cast::<$features>().as_mut_unchecked();
                                let other = other.cast::<$features>().as_ref_unchecked();

                                $(feature.$entry |= other.$entry;)*
                            }
                        ),*
                        ty => panic!(
                            "merge_features2 was supplied an invalid/unsupported PhysicalDeviceFeatures2 p_next chain ({ty:?})",
                        ),
                    }
                    feature.p_next.cast_mut();
                }

                for (ty, feature_ptr) in other_types {
                    match ty {
                        $(
                            <$features>::STRUCTURE_TYPE => {
                                let mut feature = feature_ptr.cast::<$features>().read();

                                feature.p_next = target.p_next.cast();
                                target.p_next = Box::into_raw(Box::new(feature)).cast();
                            }
                        ),*
                        ty => {
                            warn!(?ty, "merge_features2 encountered an unknown StructureType");
                        }
                    }
                }
            }
        }

        fn all_features2(
            lhs: &ash::vk::PhysicalDeviceFeatures2,
            rhs: &ash::vk::PhysicalDeviceFeatures2,
        ) -> bool {
            unsafe {
                let mut rhs_types = Vec::new();

                let mut p_next = rhs.p_next.cast::<ash::vk::BaseInStructure>();
                while !p_next.is_null() {
                    let feature = p_next.read();
                    rhs_types.push(p_next);
                    p_next = feature.p_next.cast_mut();
                }
                rhs_types.sort_unstable_by_key(|feature| feature.read().s_type);

                let mut p_next = lhs.p_next.cast::<ash::vk::BaseInStructure>();
                while !p_next.is_null() {
                    let feature = p_next.read();
                    let Ok(index) = rhs_types
                        .binary_search_by_key(&feature.s_type, |feature| feature.read().s_type)
                    else {
                        p_next = feature.p_next.cast_mut();
                        continue;
                    };
                    let other = rhs_types[index];

                    match feature.s_type {
                        $(
                            <$features>::STRUCTURE_TYPE => {
                                let feature = p_next.cast::<$features>().as_mut_unchecked();
                                let other = other.cast::<$features>().as_ref_unchecked();

                                $(
                                    if feature.$entry < other.$entry {
                                        tracing::trace!("Feature {} failed all_features2 check: {} {}", stringify!($entry), feature.$entry, other.$entry);
                                        return false;
                                    }
                                )*
                            }
                        ),*
                        ty => panic!(
                            "all_features2 was supplied an invalid/unsupported PhysicalDeviceFeatures2 p_next chain ({ty:?})",
                        ),
                    }

                    p_next = feature.p_next.cast_mut();
                }

                true
            }
        }

        fn select_features2(
            target: &mut ash::vk::PhysicalDeviceFeatures2,
            select: &ash::vk::PhysicalDeviceFeatures2,
        ) {
            unsafe {
                let mut select_types = Vec::new();

                let mut p_next = select.p_next.cast::<ash::vk::BaseInStructure>();
                while !p_next.is_null() {
                    let feature = p_next.read();
                    select_types.push(p_next);
                    p_next = feature.p_next.cast_mut();
                }
                select_types.sort_unstable_by_key(|feature| feature.read().s_type);

                let mut p_next = target.p_next.cast::<ash::vk::BaseInStructure>();
                while !p_next.is_null() {
                    let feature = p_next.read();
                    let index = select_types
                        .binary_search_by_key(&feature.s_type, |feature| feature.read().s_type).ok();

                    match feature.s_type {
                        $(
                            <$features>::STRUCTURE_TYPE => {
                                let feature = p_next.cast::<$features>().as_mut_unchecked();
                                let select = if let Some(index) = index {
                                    let select_ptr = select_types[index];
                                    select_ptr.cast::<$features>().read()
                                } else {
                                    <$features>::default()
                                };

                                //tracing::trace!(?feature, ?select);
                                $(
                                    if select.$entry == 0 {
                                        feature.$entry = 0;
                                    }
                                )*
                            }
                        ),*
                        ty => panic!(
                            "select_features2 was supplied an invalid/unsupported PhysicalDeviceFeatures2 p_next chain ({ty:?})",
                        ),
                    }

                    p_next = feature.p_next.cast_mut();
                }
            }
        }

        fn zeroed_features2<'a>(
            src: &ash::vk::PhysicalDeviceFeatures2
        ) -> ash::vk::PhysicalDeviceFeatures2<'a> {
            unsafe {
                let mut new = ash::vk::PhysicalDeviceFeatures2::default().features(src.features);

                let mut p_next = src.p_next.cast::<ash::vk::BaseInStructure>();
                while !p_next.is_null() {
                    let feature = p_next.read();
                    match feature.s_type {
                        $(
                            <$features>::STRUCTURE_TYPE => {
                                let mut feature = <$features>::default();
                                feature.p_next = new.p_next;
                                new.p_next = Box::into_raw(Box::new(feature)).cast();
                            }
                        ),*
                        ty => panic!(
                            "zeroed_features2 was supplied an invalid/unsupported PhysicalDeviceFeatures2 p_next chain ({ty:?})",
                        ),
                    }
                    p_next = feature.p_next.cast_mut();
                }

                new
            }
        }

        unsafe fn print_features2(features: &ash::vk::PhysicalDeviceFeatures2) {
            unsafe {
                let mut p_next = features.p_next.cast::<ash::vk::BaseInStructure>();
                let span = tracing::trace_span!("PhysicalDeviceFeatures2");
                let _guard = span.enter();
                while !p_next.is_null() {
                    let feature = p_next.read();
                    match feature.s_type {
                        $(
                            <$features>::STRUCTURE_TYPE => {
                                let feature = p_next.cast::<$features>().read();
                                tracing::trace!(?feature);
                            }
                        ),*
                        ty => panic!(
                            "destroy_features2 was supplied an invalid/unsupported PhysicalDeviceFeatures2 p_next chain ({ty:?})",
                        ),
                    }
                    p_next = feature.p_next.cast_mut();
                }
            }
        }

        //fn consolidate_features2<'a>(
        //    src: &ash::vk::PhysicalDeviceFeatures2
        //) -> ash::vk::PhysicalDeviceFeatures2<'a> {
        //    unsafe {
        //        let mut new = ash::vk::PhysicalDeviceFeatures2::default().features(src.features);
        //        let vulkan11 = ash::vk::PhysicalDeviceVulkan11Features::default();
        //        let vulkan12 = ash::vk::PhysicalDeviceVulkan12Features::default();
        //        let vulkan13 = ash::vk::PhysicalDeviceVulkan13Features::default();
        //        let vulkan14 = PhysicalDeviceVulkan14Features::default();
        //
        //        let mut p_next = src.p_next.cast::<ash::vk::BaseInStructure>();
        //        while !p_next.is_null() {
        //            let feature = p_next.read();
        //            match feature.s_type {
        //                $(
        //                    <$features>::STRUCTURE_TYPE => {
        //                        let mut feature = <$features>::default();
        //                        feature.p_next = new.p_next;
        //                        new.p_next = Box::into_raw(Box::new(feature)).cast();
        //                    }
        //                ),*
        //                ty => panic!(
        //                    "consolidate_features2 was supplied an invalid/unsupported PhysicalDeviceFeatures2 p_next chain ({ty:?})",
        //                ),
        //            }
        //            p_next = feature.p_next.cast_mut();
        //        }
        //
        //        new
        //    }
        //}
    };
}

features2_operations!(
    ash::vk::PhysicalDeviceDescriptorIndexingFeaturesEXT[shader_input_attachment_array_dynamic_indexing,shader_uniform_texel_buffer_array_dynamic_indexing,shader_storage_texel_buffer_array_dynamic_indexing,shader_uniform_buffer_array_non_uniform_indexing,shader_sampled_image_array_non_uniform_indexing,shader_storage_buffer_array_non_uniform_indexing,shader_storage_image_array_non_uniform_indexing,shader_input_attachment_array_non_uniform_indexing,shader_uniform_texel_buffer_array_non_uniform_indexing,shader_storage_texel_buffer_array_non_uniform_indexing,descriptor_binding_uniform_buffer_update_after_bind,descriptor_binding_sampled_image_update_after_bind,descriptor_binding_storage_image_update_after_bind,descriptor_binding_storage_buffer_update_after_bind,descriptor_binding_uniform_texel_buffer_update_after_bind,descriptor_binding_storage_texel_buffer_update_after_bind,descriptor_binding_update_unused_while_pending,descriptor_binding_partially_bound,descriptor_binding_variable_descriptor_count,runtime_descriptor_array],
    ash::vk::PhysicalDeviceTimelineSemaphoreFeaturesKHR[timeline_semaphore],
    ash::vk::PhysicalDeviceImageRobustnessFeaturesEXT[robust_image_access],
    ash::vk::PhysicalDeviceRobustness2FeaturesEXT[robust_buffer_access2,robust_image_access2,null_descriptor],
    ash::vk::PhysicalDeviceMultiviewFeatures[multiview,multiview_geometry_shader,multiview_tessellation_shader],
    ash::vk::PhysicalDeviceSamplerYcbcrConversionFeatures[sampler_ycbcr_conversion],
    ash::vk::PhysicalDeviceTextureCompressionASTCHDRFeaturesEXT[texture_compression_astc_hdr],
    ash::vk::PhysicalDeviceShaderFloat16Int8Features[shader_float16,shader_int8],
    ash::vk::PhysicalDevice16BitStorageFeatures[storage_buffer16_bit_access,uniform_and_storage_buffer16_bit_access,storage_push_constant16,storage_input_output16],
    ash::vk::PhysicalDeviceAccelerationStructureFeaturesKHR[acceleration_structure,acceleration_structure_capture_replay,acceleration_structure_indirect_build,acceleration_structure_host_commands],
    ash::vk::PhysicalDeviceBufferDeviceAddressFeaturesKHR[buffer_device_address,buffer_device_address_capture_replay,buffer_device_address_multi_device],
    ash::vk::PhysicalDeviceRayQueryFeaturesKHR[ray_query],
    ash::vk::PhysicalDeviceRayTracingPipelineFeaturesKHR[ray_tracing_pipeline,ray_tracing_pipeline_shader_group_handle_capture_replay,ray_tracing_pipeline_shader_group_handle_capture_replay_mixed,ray_tracing_pipeline_trace_rays_indirect,ray_traversal_primitive_culling],
    ash::vk::PhysicalDeviceZeroInitializeWorkgroupMemoryFeatures[shader_zero_initialize_workgroup_memory],
    ash::vk::PhysicalDeviceRayTracingPositionFetchFeaturesKHR[ray_tracing_position_fetch],
    ash::vk::PhysicalDeviceShaderAtomicInt64Features[shader_buffer_int64_atomics,shader_shared_int64_atomics],
    ash::vk::PhysicalDeviceShaderImageAtomicInt64FeaturesEXT[shader_image_int64_atomics,sparse_image_int64_atomics],
    ash::vk::PhysicalDeviceShaderAtomicFloatFeaturesEXT[shader_buffer_float32_atomics,shader_buffer_float32_atomic_add,shader_buffer_float64_atomics,shader_buffer_float64_atomic_add,shader_shared_float32_atomics,shader_shared_float32_atomic_add,shader_shared_float64_atomics,shader_shared_float64_atomic_add,shader_image_float32_atomics,shader_image_float32_atomic_add,sparse_image_float32_atomics,sparse_image_float32_atomic_add],
    ash::vk::PhysicalDeviceSubgroupSizeControlFeatures[subgroup_size_control,compute_full_subgroups],
    ash::vk::PhysicalDeviceMaintenance4FeaturesKHR[maintenance4],
    ash::vk::PhysicalDeviceMeshShaderFeaturesEXT[task_shader,mesh_shader,multiview_mesh_shader,primitive_fragment_shading_rate_mesh_shader,mesh_shader_queries],
    ash::vk::PhysicalDeviceShaderIntegerDotProductFeaturesKHR[shader_integer_dot_product],
    ash::vk::PhysicalDeviceFragmentShaderBarycentricFeaturesKHR[fragment_shader_barycentric],
    ash::vk::PhysicalDevicePortabilitySubsetFeaturesKHR[constant_alpha_color_blend_factors,events,image_view_format_reinterpretation,image_view_format_swizzle,image_view2_d_on3_d_image,multisample_array_image,mutable_comparison_samplers,point_polygons,sampler_mip_lod_bias,separate_stencil_mask_ref,shader_sample_rate_interpolation_functions,tessellation_isolines,tessellation_point_mode,triangle_fans,vertex_attribute_access_beyond_stride],
    ash::vk::PhysicalDeviceCooperativeMatrixFeaturesKHR[cooperative_matrix,cooperative_matrix_robust_buffer_access],
    ash::vk::PhysicalDeviceVulkanMemoryModelFeaturesKHR[vulkan_memory_model,vulkan_memory_model_availability_visibility_chains,vulkan_memory_model_device_scope],
    ash::vk::PhysicalDeviceShaderDrawParametersFeatures[shader_draw_parameters],
    ash::vk::PhysicalDeviceVulkan11Features[storage_buffer16_bit_access,uniform_and_storage_buffer16_bit_access,storage_push_constant16,storage_input_output16,multiview,multiview_geometry_shader,multiview_tessellation_shader,variable_pointers,variable_pointers_storage_buffer,protected_memory,sampler_ycbcr_conversion,shader_draw_parameters],
    ash::vk::PhysicalDeviceVulkan12Features[sampler_mirror_clamp_to_edge,draw_indirect_count,storage_buffer8_bit_access,uniform_and_storage_buffer8_bit_access,storage_push_constant8,shader_buffer_int64_atomics,shader_shared_int64_atomics,shader_float16,shader_int8,descriptor_indexing,shader_input_attachment_array_dynamic_indexing,shader_uniform_texel_buffer_array_dynamic_indexing,shader_storage_texel_buffer_array_dynamic_indexing,shader_uniform_buffer_array_non_uniform_indexing,shader_sampled_image_array_non_uniform_indexing,shader_storage_buffer_array_non_uniform_indexing,shader_storage_image_array_non_uniform_indexing,shader_input_attachment_array_non_uniform_indexing,shader_uniform_texel_buffer_array_non_uniform_indexing,shader_storage_texel_buffer_array_non_uniform_indexing,descriptor_binding_uniform_buffer_update_after_bind,descriptor_binding_sampled_image_update_after_bind,descriptor_binding_storage_image_update_after_bind,descriptor_binding_storage_buffer_update_after_bind,descriptor_binding_uniform_texel_buffer_update_after_bind,descriptor_binding_storage_texel_buffer_update_after_bind,descriptor_binding_update_unused_while_pending,descriptor_binding_partially_bound,descriptor_binding_variable_descriptor_count,runtime_descriptor_array,sampler_filter_minmax,scalar_block_layout,imageless_framebuffer,uniform_buffer_standard_layout,shader_subgroup_extended_types,separate_depth_stencil_layouts,host_query_reset,timeline_semaphore,buffer_device_address,buffer_device_address_capture_replay,buffer_device_address_multi_device,vulkan_memory_model,vulkan_memory_model_device_scope,vulkan_memory_model_availability_visibility_chains,shader_output_viewport_index,shader_output_layer,subgroup_broadcast_dynamic_id],
    ash::vk::PhysicalDeviceVulkan13Features[robust_image_access,inline_uniform_block,descriptor_binding_inline_uniform_block_update_after_bind,pipeline_creation_cache_control,private_data,shader_demote_to_helper_invocation,shader_terminate_invocation,subgroup_size_control,compute_full_subgroups,synchronization2,texture_compression_astc_hdr,shader_zero_initialize_workgroup_memory,dynamic_rendering,shader_integer_dot_product,maintenance4],
    PhysicalDeviceVulkan14Features[global_priority_query,shader_subgroup_rotate,shader_subgroup_rotate_clustered,shader_float_controls2,shader_expect_assume,rectangular_lines,bresenham_lines,smooth_lines,stippled_rectangular_lines,stippled_bresenham_lines,stippled_smooth_lines,vertex_attribute_instance_rate_divisor,vertex_attribute_instance_rate_zero_divisor,index_type_uint8,dynamic_rendering_local_read,maintenance5,maintenance6,pipeline_protected_access,pipeline_robustness,host_image_copy,push_descriptor],
    ash::vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT[swapchain_maintenance1]
);

pub struct VulkanBackend<'a> {
    instance: wgpu::Instance,
    adapter:  wgpu::Adapter,
    device:   wgpu::Device,
    queue:    wgpu::Queue,

    instance_extensions: Vec<*const i8>, // CStr isn't ABI compatible with const char*
    device_extensions:   Vec<*const i8>,

    device_features: ash::vk::PhysicalDeviceFeatures2<'a>,
    device_queues:   Vec<ffmpeg::ffi::AVVulkanDeviceQueueFamily>,
}

impl<'a> VulkanBackend<'a> {
    pub fn egui_existing(&self) -> egui_wgpu::WgpuSetupExisting {
        egui_wgpu::WgpuSetupExisting {
            instance: self.instance.clone(),
            adapter:  self.adapter.clone(),
            device:   self.device.clone(),
            queue:    self.queue.clone(),
        }
    }

    unsafe extern "C" fn free_vulkan_hwdevice(hwdevice_ctx: *mut ffmpeg::ffi::AVHWDeviceContext) {
        unsafe {
            let Some(hwdevice_ctx) = hwdevice_ctx.as_mut() else {
                warn!(
                    "VulkanBackend::free_vulkan_hwdevice was supplied nullptr instead of hwdevice_ctx"
                );
                return;
            };

            let vk_backend = hwdevice_ctx.user_opaque.cast::<VulkanBackend>();
            if vk_backend.is_null() {
                return;
            }
            _ = Rc::from_raw(vk_backend);
        }
    }

    fn update_extensions(
        dst: &mut Vec<&'static CStr>,
        src: impl Iterator<Item = &'static CStr>,
        supported: &[ash::vk::ExtensionProperties],
    ) {
        let mut supported = supported
            .iter()
            .filter_map(|ext| ext.extension_name_as_c_str().ok())
            .collect::<Vec<_>>();
        supported.sort_unstable();

        // Add to Instance extensions if supported & not already added
        dst.sort_unstable();
        for extension in src {
            if supported.binary_search(&extension).is_ok()
                && let Err(index) = dst.binary_search(&extension)
            {
                dst.insert(index, extension);
            }
        }

        dst.dedup(); // Ensure we aren't requesting multiple of the same extension
    }

    fn ffmpeg_optional_instance_extensions() -> impl Iterator<Item = &'static CStr> {
        unsafe {
            let mut nb_ffmpeg_extensions = 0;
            let ffmpeg_extensions =
                ffmpeg::ffi::av_vk_get_optional_instance_extensions(&raw mut nb_ffmpeg_extensions);

            let nb_ffmpeg_extensions = usize::try_from(nb_ffmpeg_extensions)
                .expect("FFMpeg requested more than usize::MAX instance extensions");
            std::slice::from_raw_parts(ffmpeg_extensions, nb_ffmpeg_extensions)
                .iter()
                .map(|ptr| std::mem::transmute::<&'_ _, &'static _>(CStr::from_ptr(*ptr)))
        }
    }

    fn ffmpeg_optional_device_extensions() -> impl Iterator<Item = &'static CStr> {
        unsafe {
            let mut nb_ffmpeg_extensions = 0;
            let ffmpeg_extensions =
                ffmpeg::ffi::av_vk_get_optional_device_extensions(&raw mut nb_ffmpeg_extensions);

            let nb_ffmpeg_extensions = usize::try_from(nb_ffmpeg_extensions)
                .expect("FFMpeg requested more than usize::MAX device extensions");
            std::slice::from_raw_parts(ffmpeg_extensions, nb_ffmpeg_extensions)
                .iter()
                .map(|ptr| std::mem::transmute::<&'_ _, &'static _>(CStr::from_ptr(*ptr)))
        }
    }

    unsafe fn list_queue_families(
        instance: &ash::Instance,
        phys_device: ash::vk::PhysicalDevice,
    ) -> Box<[ffmpeg::ffi::AVVulkanDeviceQueueFamily]> {
        unsafe {
            let qf_len = instance.get_physical_device_queue_family_properties2_len(phys_device);
            let mut video_props = vec![ash::vk::QueueFamilyVideoPropertiesKHR::default(); qf_len];
            let mut qf_props = video_props
                .iter_mut()
                .map(|vp| ash::vk::QueueFamilyProperties2::default().push_next(vp))
                .collect::<Vec<_>>();
            instance.get_physical_device_queue_family_properties2(phys_device, &mut qf_props);

            qf_props
                .into_iter()
                .enumerate()
                .map(|(idx, props)| {
                    let mut qf = ffmpeg::ffi::AVVulkanDeviceQueueFamily {
                        idx:        idx as i32,
                        num:        props.queue_family_properties.queue_count.cast_signed(),
                        flags:      props.queue_family_properties.queue_flags.as_raw(),
                        video_caps: 0,
                    };

                    if let Some(p_next) = props
                        .p_next
                        .cast::<ash::vk::QueueFamilyVideoPropertiesKHR>()
                        .as_ref()
                    {
                        qf.video_caps = p_next.video_codec_operations.as_raw().cast_signed();
                    }

                    qf
                })
                .collect()
        }
    }

    pub fn create_ffmpeg_hwctx(
        self: &Rc<Self>
    ) -> Result<*mut ffmpeg::ffi::AVBufferRef, HWDeviceError> {
        unsafe {
            #[allow(clippy::wildcard_imports)]
            use ffmpeg::ffi::*;

            // Convert wgpu to vulkan hal
            let Some(hal_instance) = self.instance.as_hal::<wgpu_hal::api::Vulkan>() else {
                unreachable!("VulkanBackend contains non-vulkan wgpu::Instance")
            };
            let Some(hal_device) = self.device.as_hal::<wgpu_hal::api::Vulkan>() else {
                unreachable!("VulkanBackend contains non-vulkan wgpu::Device")
            };

            let mut ctx = av_hwdevice_ctx_alloc(AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN);
            if ctx.is_null() {
                return Err(HWDeviceError::AllocationFailed);
            }

            let Some(hw_device_ctx) = ctx
                .read()
                .data
                .try_cast_aligned::<AVHWDeviceContext>()
                .expect("FFmpeg didn't align AVHWDeviceContext")
                .as_mut()
            else {
                av_buffer_unref(&raw mut ctx);
                return Err(HWDeviceError::AllocationFailed);
            };

            let Some(vk_device_ctx) = hw_device_ctx
                .hwctx
                .try_cast_aligned::<AVVulkanDeviceContext>()
                .expect("FFmpeg didn't align AVVulkanDeviceContext")
                .as_mut()
            else {
                av_buffer_unref(&raw mut ctx);
                return Err(HWDeviceError::AllocationFailed);
            };

            #[allow(clippy::missing_transmute_annotations)]
            let get_proc_addr = Some(std::mem::transmute(
                hal_instance
                    .shared_instance()
                    .entry()
                    .static_fn()
                    .get_instance_proc_addr,
            ));
            vk_device_ctx.get_proc_addr = get_proc_addr;

            vk_device_ctx.inst = std::mem::transmute::<ash::vk::Instance, VkInstance>(
                hal_instance.shared_instance().raw_instance().handle(),
            );
            vk_device_ctx.phys_dev = std::mem::transmute::<ash::vk::PhysicalDevice, VkPhysicalDevice>(
                hal_device.raw_physical_device(),
            );
            vk_device_ctx.act_dev =
                std::mem::transmute::<ash::vk::Device, VkDevice>(hal_device.raw_device().handle());

            vk_device_ctx.device_features = std::mem::transmute::<
                ash::vk::PhysicalDeviceFeatures2,
                ffmpeg::ffi::VkPhysicalDeviceFeatures2,
            >(self.device_features);

            vk_device_ctx.enabled_inst_extensions = self.instance_extensions.as_ptr();
            vk_device_ctx.nb_enabled_dev_extensions = self.instance_extensions.len() as i32;

            vk_device_ctx.enabled_dev_extensions = self.device_extensions.as_ptr();
            vk_device_ctx.nb_enabled_dev_extensions = self.device_extensions.len() as i32;

            vk_device_ctx.nb_qf = self.device_queues.len() as i32;
            for (i, qf) in self.device_queues.iter().enumerate() {
                vk_device_ctx.qf[i] = *qf;
            }
            vk_device_ctx.queue_flags = ash::vk::DeviceQueueCreateFlags::empty().as_raw();

            vk_device_ctx.alloc = std::ptr::null();
            vk_device_ctx.lock_queue = None;
            vk_device_ctx.unlock_queue = None;

            hw_device_ctx.free = Some(Self::free_vulkan_hwdevice);
            hw_device_ctx.user_opaque = Rc::into_raw(self.clone())
                .cast::<std::ffi::c_void>()
                .cast_mut();

            //debug!(?hw_device_ctx, ?vk_device_ctx);
            let err = av_hwdevice_ctx_init(ctx);
            if err != 0 {
                return Err(ffmpeg::Error::from(err).into());
            }

            Ok(ctx)
        }
    }

    pub fn create_libplacebo_ctx<'placebo: 'a>(
        self: &Rc<Self>,
        log: libplacebo::Log,
    ) -> Result<libplacebo::vulkan::GpuVk<'placebo>, HWDeviceError> {
        unsafe {
            // Convert wgpu to vulkan hal
            let Some(hal_instance) = self.instance.as_hal::<wgpu_hal::api::Vulkan>() else {
                unreachable!("VulkanBackend contains non-vulkan wgpu::Instance")
            };
            let Some(hal_device) = self.device.as_hal::<wgpu_hal::api::Vulkan>() else {
                unreachable!("VulkanBackend contains non-vulkan wgpu::Device")
            };

            let device_extensions = self
                .device_extensions
                .iter()
                .map(|ptr| CStr::from_ptr(*ptr))
                .collect::<Vec<_>>();

            let mut qf_iter = self.device_queues.iter();
            let graphics_queue = qf_iter
                .find(|family| {
                    ash::vk::QueueFlags::from_raw(family.flags)
                        .contains(ash::vk::QueueFlags::GRAPHICS)
                })
                .copied()
                .expect("This function should not be able to be called without a Graphics QF");
            let compute_queue = qf_iter
                .find(|family| {
                    ash::vk::QueueFlags::from_raw(family.flags)
                        .contains(ash::vk::QueueFlags::COMPUTE)
                })
                .copied();
            let transfer_queue = qf_iter
                .find(|family| {
                    ash::vk::QueueFlags::from_raw(family.flags)
                        .contains(ash::vk::QueueFlags::TRANSFER)
                })
                .copied();

            let params = libplacebo::vulkan::InitParamsExisting {
                entry:              hal_instance.shared_instance().entry().clone(),
                instance:           hal_instance.shared_instance().raw_instance().handle(),
                phys_device:        hal_device.raw_physical_device(),
                device:             hal_device.raw_device().handle(),
                device_extensions:  &device_extensions,
                device_features:    self.device_features,
                graphics_queue:     libplacebo::vulkan::VkQueueFamily {
                    index: graphics_queue.idx.cast_unsigned(),
                    count: graphics_queue.num.cast_unsigned(),
                },
                compute_queue:      compute_queue.map(|queue| libplacebo::vulkan::VkQueueFamily {
                    index: queue.idx.cast_unsigned(),
                    count: queue.num.cast_unsigned(),
                }),
                transfer_queue:     transfer_queue.map(|queue| libplacebo::vulkan::VkQueueFamily {
                    index: queue.idx.cast_unsigned(),
                    count: queue.num.cast_unsigned(),
                }),
                disable_compute:    false,
                max_glsl_version:   u32::MAX.cast_signed(), // Any version should be fine
                max_vk_api_version: u32::MAX,               // Same with vulkan
            };

            libplacebo::vulkan::GpuVk::import(log, &params)
                .ok_or(HWDeviceError::LibPlacebo("pl_vulkan_import"))
        }
    }

    // Select required queues for egui, ffmpeg & libplacebo
    fn select_queues(
        queue_families: &[ffmpeg::ffi::AVVulkanDeviceQueueFamily]
    ) -> Result<Vec<ffmpeg::ffi::AVVulkanDeviceQueueFamily>, WgpuInitError> {
        let Some(graphics_qf) = queue_families.iter().find(|qf| {
            ash::vk::QueueFlags::from_raw(qf.flags).contains(ash::vk::QueueFlags::GRAPHICS)
        }) else {
            return Err(WgpuInitError::DeviceGraphicsQueue);
        };

        let mut families = vec![*graphics_qf];

        let wanted_queues = [
            ash::vk::QueueFlags::VIDEO_DECODE_KHR,
            ash::vk::QueueFlags::COMPUTE,
            ash::vk::QueueFlags::TRANSFER,
        ];

        for wanted in wanted_queues {
            let Some(family) = queue_families
                .iter()
                .filter(|family| !families.contains(family))
                .find(|family| ash::vk::QueueFlags::from_raw(family.flags).contains(wanted))
            else {
                continue;
            };

            families.push(*family);
        }

        Ok(families)
    }

    unsafe fn select_features<'b>(
        mut wgpu_features: wgpu_hal::vulkan::PhysicalDeviceFeatures,
        instance: &ash::Instance,
        physical_device: ash::vk::PhysicalDevice,
    ) -> Result<ash::vk::PhysicalDeviceFeatures2<'b>, WgpuInitError> {
        unsafe {
            let placebo_required = libplacebo::vulkan::GpuVk::required_device_features();
            let placebo_recommended = libplacebo::vulkan::GpuVk::recommended_device_features();

            let device_info =
                wgpu_features.add_to_device_create(ash::vk::DeviceCreateInfo::default());
            let mut wgpu_required = ash::vk::PhysicalDeviceFeatures2::default()
                .features(device_info.p_enabled_features.read());
            wgpu_required.p_next = device_info.p_next.cast_mut();

            // Merge features
            let mut features = ash::vk::PhysicalDeviceFeatures2::default();
            merge_features2(&mut features, &wgpu_required);
            merge_features2(&mut features, &placebo_required);
            merge_features2(&mut features, &placebo_recommended);

            let mut available_features = zeroed_features2(&features);
            instance.get_physical_device_features2(physical_device, &mut available_features);
            select_features2(&mut features, &available_features);
            destroy_features2(&mut available_features);

            if !all_features2(&features, &wgpu_required) {
                destroy_features2(&mut features);
                return Err(WgpuInitError::PhysicalDeviceUnsupported);
            }
            if !all_features2(&features, &placebo_required) {
                destroy_features2(&mut features);
                return Err(WgpuInitError::PhysicalDeviceUnsupported);
            }

            Ok(features)
        }
    }

    #[allow(clippy::too_many_lines)]
    pub fn new() -> Result<Rc<Self>, WgpuInitError> {
        unsafe {
            let instance_flags = wgpu::InstanceFlags::from_env_or_default();

            let hal_instance = wgpu_hal::vulkan::Instance::init_with_callback(
                &wgpu_hal::InstanceDescriptor {
                    name:                     "wgpu_vulkan_instance",
                    flags:                    instance_flags,
                    memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
                    backend_options:          wgpu::BackendOptions::from_env_or_default(),
                    telemetry:                None,
                    display:                  None,
                },
                Some(Box::new(|args| {
                    let ffmpeg_extensions = Self::ffmpeg_optional_instance_extensions();

                    let Ok(supported) = args.entry.enumerate_instance_extension_properties(None)
                    else {
                        warn!(
                            "Was unable to get list of supported instance extensions during Instance creation"
                        );
                        return;
                    };

                    Self::update_extensions(args.extensions, ffmpeg_extensions, &supported);
                })),
            )?;

            let version = hal_instance.shared_instance().instance_api_version();
            let ver_major = ash::vk::api_version_major(version);
            let ver_minor = ash::vk::api_version_minor(version);
            if ver_major == 1 && ver_minor < 3 {
                return Err(WgpuInitError::VulkanVersion(std::format!(
                    "{ver_major}.{ver_minor}"
                )));
            }

            let exposed_adapters = hal_instance.enumerate_adapters(None);
            let phys_device = if let Ok(device_name) = std::env::var("WGPU_DEVICE_NAME") {
                exposed_adapters
                    .iter()
                    .find(|adapter| adapter.info.name == device_name)
                    .ok_or(WgpuInitError::RequestedPhysicalDevice(device_name))?
                    .adapter
                    .raw_physical_device()
            } else {
                let mut index = 0;
                for (i, adapter) in exposed_adapters.iter().enumerate() {
                    if !adapter.features.intersects(
                        wgpu::Features::VULKAN_EXTERNAL_MEMORY_FD
                            | wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32,
                    ) {
                        continue;
                    }
                    if adapter.info.device_type == wgpu::DeviceType::DiscreteGpu {
                        index = i;
                    }
                }

                exposed_adapters[index].adapter.raw_physical_device()
            };
            let exposed_adapter = hal_instance
                .expose_adapter(phys_device)
                .expect("PhysicalDevice suddenly no longer available");

            // Figure out queues that we want
            let queue_families = Self::list_queue_families(
                hal_instance.shared_instance().raw_instance(),
                phys_device,
            );
            let selected_queues = Self::select_queues(&queue_families)?;

            // Manually create Device from ash directly as wgpu-hal doesn't properly expose
            // Features
            let mut device_create_info = ash::vk::DeviceCreateInfo::default();

            let mut device_extensions = exposed_adapter
                .adapter
                .required_device_extensions(exposed_adapter.features);
            let wgpu_features = exposed_adapter
                .adapter
                .physical_device_features(&device_extensions, exposed_adapter.features);

            if let Ok(supported) = hal_instance
                .shared_instance()
                .raw_instance()
                .enumerate_device_extension_properties(phys_device)
            {
                Self::update_extensions(
                    &mut device_extensions,
                    Self::ffmpeg_optional_device_extensions(),
                    &supported,
                );
                Self::update_extensions(
                    &mut device_extensions,
                    [c"VK_KHR_video_queue", c"VK_KHR_video_decode_queue"].into_iter(), /* Request video decode extensions (if available) */
                    &supported,
                );
                Self::update_extensions(
                    &mut device_extensions,
                    libplacebo::vulkan::GpuVk::recommended_device_extensions(),
                    &supported,
                );
            }

            let str_pointers = device_extensions
                .iter()
                .map(|&s| s.as_ptr())
                .collect::<Vec<_>>();
            device_create_info = device_create_info.enabled_extension_names(&str_pointers);

            let queue_create_infos = selected_queues
                .iter()
                .copied()
                .map(|qf| {
                    ash::vk::DeviceQueueCreateInfo::default()
                        .queue_family_index(qf.idx.cast_unsigned())
                        .queue_priorities(std::mem::transmute::<&'_ _, &'_ _>(
                            vec![1.0f32; qf.num.cast_unsigned() as usize].as_slice(),
                        ))
                })
                .collect::<Vec<_>>();
            let Some(graphics_queue) = selected_queues.iter().find(|qf| {
                ash::vk::QueueFlags::from_raw(qf.flags).contains(ash::vk::QueueFlags::GRAPHICS)
            }) else {
                return Err(WgpuInitError::DeviceGraphicsQueue);
            };
            device_create_info = device_create_info.queue_create_infos(&queue_create_infos);

            let mut device_features = Self::select_features(
                wgpu_features,
                hal_instance.shared_instance().raw_instance(),
                phys_device,
            )?;
            device_create_info = device_create_info.enabled_features(&device_features.features);
            device_create_info.p_next = device_features.p_next;

            // FIXME: Make sure PhysicalDeviceFeatures2 is cleanly dropped if create_device
            // or device_from_raw fail
            let raw_device = match hal_instance.shared_instance().raw_instance().create_device(
                phys_device,
                &device_create_info,
                None,
            ) {
                Ok(dev) => Ok(dev),
                Err(vkerror) => {
                    destroy_features2(&mut device_features);
                    Err(WgpuInitError::DeviceOpen(vkerror))
                },
            }?;
            let hal_device = match exposed_adapter.adapter.device_from_raw(
                raw_device,
                None,
                &device_extensions,
                exposed_adapter.features,
                &exposed_adapter.capabilities.limits,
                &wgpu::MemoryHints::default(),
                graphics_queue.idx.cast_unsigned(),
                0,
            ) {
                Ok(hal_device) => hal_device,
                Err(error) => {
                    destroy_features2(&mut device_features);
                    return Err(error)?;
                },
            };

            // Copy what Instance & Device creation does to select flags
            let Ok(mut instance_extensions) = wgpu_hal::vulkan::Instance::desired_extensions(
                hal_instance.shared_instance().entry(),
                version,
                instance_flags,
            ) else {
                unreachable!("Instance::init_with_callback passed this");
            };

            if let Ok(supported) = hal_instance
                .shared_instance()
                .entry()
                .enumerate_instance_extension_properties(None)
            {
                Self::update_extensions(
                    &mut instance_extensions,
                    Self::ffmpeg_optional_instance_extensions(),
                    &supported,
                );
            }

            let instance = wgpu::Instance::from_hal::<wgpu_hal::api::Vulkan>(hal_instance);
            let adapter = instance.create_adapter_from_hal(exposed_adapter);
            let (device, queue) =
                adapter.create_device_from_hal(hal_device, &wgpu::DeviceDescriptor {
                    label:                 Some("wgpu_vulkan_device"),
                    required_features:     cfg_select! {
                        unix => wgpu::Features::VULKAN_EXTERNAL_MEMORY_FD,
                        windows => wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32,
                        _ => wgpu::Features::empty(),
                    },
                    required_limits:       wgpu::Limits::defaults(),
                    experimental_features: wgpu::ExperimentalFeatures::default(),
                    memory_hints:          wgpu::MemoryHints::default(),
                    trace:                 wgpu::Trace::default(),
                })?;

            Ok(Rc::new(Self {
                instance,
                adapter,
                device,
                queue,
                instance_extensions: instance_extensions.into_iter().map(CStr::as_ptr).collect(),
                device_extensions: device_extensions.into_iter().map(CStr::as_ptr).collect(),
                device_features,
                device_queues: selected_queues,
            }))
        }
    }
}
