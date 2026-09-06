#include <iostream>
#include <stdexcept>
#include <cstdlib>
#include <set>
#include <algorithm>

#define VULKAN_HPP_NO_SETTERS
#define VULKAN_HPP_NO_STRUCT_CONSTRUCTORS
#include <vulkan/vulkan_raii.hpp>

extern "C"
{
#include <libavutil/hwcontext_vulkan.h>
#include <libplacebo/vulkan.h>
}

class AppException : std::exception
{
private:
    const char *mWhat = "Unknown Error";

public:
    AppException(VkResult vkResult)
    {
        switch (vkResult)
        {
        case VK_ERROR_LAYER_NOT_PRESENT: this->mWhat = "Vulkan Error: Layer Not Present"; break;
        case VK_ERROR_OUT_OF_DEVICE_MEMORY: this->mWhat = "Vulkan Error: Device OOM"; break;
        case VK_ERROR_OUT_OF_HOST_MEMORY: this->mWhat = "Vulkan Error: Host OOM"; break;
        case VK_ERROR_VALIDATION_FAILED: this->mWhat = "Vulkan Error: Validation Failed"; break;
        default: break;
        }
    }

    const char *what() const noexcept override { return this->mWhat; }
};

template<typename T>
class ManualRaiiWrapper
{
public:
    ManualRaiiWrapper(T *ptr, void (*free)(void *)) : ptr(ptr), _free(free) { }
    ~ManualRaiiWrapper() { (_free)(ptr); }

    T *ptr;

private:
    void (*_free)(void *);
};

class Application
{
public:
    void run()
    {
        std::cout << "Hello World!" << std::endl;
        initVulkan();
    }

private:
    void initVulkan()
    {
        std::set<const char *> requestedExtensions;

        int  nb_extensions = 0;
        auto ffmpeg_extensions =
          ManualRaiiWrapper(av_vk_get_optional_instance_extensions(&nb_extensions), av_free);

        for (auto i = 0; i < nb_extensions; i++)
        {
            requestedExtensions.insert(ffmpeg_extensions.ptr[i]);
        }

        std::vector<const char *> extensions;

        auto instanceProperties = _context.enumerateInstanceExtensionProperties();

        for (auto property : instanceProperties)
        {
            auto extensionName = property.extensionName;
            if (requestedExtensions.contains(extensionName))
            {
                extensions.push_back(extensionName);
            }
        }

        constexpr vk::ApplicationInfo appInfo { .pApplicationName   = "Lowpass Analysis",
                                                .applicationVersion = VK_MAKE_VERSION(0, 1, 0),
                                                .pEngineName        = "ffmpeg+libplacebo",
                                                .engineVersion      = VK_MAKE_VERSION(1, 0, 0),
                                                .apiVersion         = vk::ApiVersion12 };

        vk::InstanceCreateInfo createInfo {
            .pApplicationInfo        = &appInfo,
            .enabledExtensionCount   = static_cast<uint32_t>(extensions.size()),
            .ppEnabledExtensionNames = extensions.data(),
        };

        _instance = vk::raii::Instance(_context, createInfo);
    }

private:
    vk::raii::Context  _context;
    vk::raii::Instance _instance = nullptr;
};

int main(int argc, char *argv[])
{
    try
    {
        Application app;
        app.run();
    }
    catch (const std::exception &e)
    {
        std::cerr << e.what() << std::endl;
        return EXIT_FAILURE;
    }

    return EXIT_SUCCESS;
}