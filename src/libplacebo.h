// Tier 4
#include <libplacebo/renderer.h>
#include <libplacebo/utils/upload.h>

#define PL_LIBAV_IMPLEMENTATION 1
#include <vulkan/vulkan.h>
#include <libplacebo/utils/libav.h>

// Tier 3/2
#include <libplacebo/shaders.h>
#include <libplacebo/shaders/custom.h>
#include <libplacebo/shaders/sampling.h>

// Tier 1
#include <libplacebo/gpu.h>
#include <libplacebo/vulkan.h>
#include <libplacebo/opengl.h>

#ifdef _WIN32
#include <libplacebo/d3d11.h>
#endif

// Tier 0
#include <libplacebo/log.h>