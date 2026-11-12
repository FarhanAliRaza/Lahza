// Lahza's shim over NVIDIA's NGX SDK (DLSS Ray Reconstruction): a C
// surface the renderer loads when it is there (crates/motion-render
// ngx.rs). Built by `just ngx` against the DLSS SDK, which is not part of
// Lahza; without this library the fast path-traced mode is not offered.
#include <vulkan/vulkan.h>
#include <string>
#include "nvsdk_ngx_vk.h"
#include "nvsdk_ngx_helpers_vk.h"
#include "nvsdk_ngx_helpers_dlssd_vk.h"

extern "C" {

struct LahzaNgxImage {
    uint64_t image, view;
    uint32_t format, width, height, pad;
};

// A frame at render size (`width` x `height`): the noisy colour (linear,
// premultiplied, coverage in alpha), diffuse and specular albedo, normals
// with roughness in alpha, linear depth, motion (to the previous frame, in
// render texels), and the output at target size. `jitter`: where in the
// texel the frame's samples went, from its centre.
struct LahzaNgxFrame {
    LahzaNgxImage color, diffuse, specular, normals, depth, motion, output;
    float jitter[2];
    uint32_t width, height, reset, pad;
};

static NVSDK_NGX_Parameter* g_params = nullptr;

// 0 when Ray Reconstruction can be used on this device.
int lahza_ngx_init(uint64_t instance, uint64_t physical, uint64_t device, void* gipa, void* gdpa, const char* snippets) {
    std::wstring dir;
    for (const char* p = snippets; *p; p++) dir.push_back((wchar_t)(unsigned char)*p);
    const wchar_t* paths[1] = {dir.c_str()};
    NVSDK_NGX_FeatureCommonInfo info{};
    info.PathListInfo.Path = paths;
    info.PathListInfo.Length = 1;
    info.LoggingInfo.MinimumLoggingLevel = NVSDK_NGX_LOGGING_LEVEL_OFF;
    NVSDK_NGX_Result r = NVSDK_NGX_VULKAN_Init(0x4c41485a41ull, L"/tmp", (VkInstance)instance, (VkPhysicalDevice)physical, (VkDevice)device,
        (PFN_vkGetInstanceProcAddr)gipa, (PFN_vkGetDeviceProcAddr)gdpa, &info, NVSDK_NGX_Version_API);
    if (NVSDK_NGX_FAILED(r)) return 1;
    if (NVSDK_NGX_FAILED(NVSDK_NGX_VULKAN_GetCapabilityParameters(&g_params))) return 2;
    int available = 0;
    g_params->Get(NVSDK_NGX_Parameter_SuperSamplingDenoising_Available, &available);
    return available == 1 ? 0 : 3;
}

// A Ray Reconstruction feature from `width` x `height` to `target`,
// recorded into `cmd`; null when it cannot be made.
void* lahza_ngx_create(uint64_t device, uint64_t cmd, uint32_t width, uint32_t height, uint32_t target_width, uint32_t target_height) {
    NVSDK_NGX_DLSSD_Create_Params cp{};
    cp.InDenoiseMode = NVSDK_NGX_DLSS_Denoise_Mode_DLUnified;
    cp.InRoughnessMode = NVSDK_NGX_DLSS_Roughness_Mode_Packed;
    cp.InUseHWDepth = NVSDK_NGX_DLSS_Depth_Type_Linear;
    cp.InWidth = width;
    cp.InHeight = height;
    cp.InTargetWidth = target_width;
    cp.InTargetHeight = target_height;
    cp.InPerfQualityValue = NVSDK_NGX_PerfQuality_Value_MaxPerf;
    cp.InFeatureCreateFlags = NVSDK_NGX_DLSS_Feature_Flags_IsHDR | NVSDK_NGX_DLSS_Feature_Flags_MVLowRes | NVSDK_NGX_DLSS_Feature_Flags_AlphaUpscaling;
    NVSDK_NGX_Handle* handle = nullptr;
    NVSDK_NGX_Result r = NGX_VULKAN_CREATE_DLSSD_EXT1((VkDevice)device, (VkCommandBuffer)cmd, 1, 1, &handle, g_params, &cp);
    return NVSDK_NGX_FAILED(r) ? nullptr : handle;
}

static NVSDK_NGX_Resource_VK resource(const LahzaNgxImage& m, bool write) {
    VkImageSubresourceRange range{VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    return NVSDK_NGX_Create_ImageView_Resource_VK((VkImageView)m.view, (VkImage)m.image, range, (VkFormat)m.format, m.width, m.height, write);
}

static void move(VkCommandBuffer cmd, const LahzaNgxImage& m, VkImageLayout from, VkImageLayout to) {
    VkImageMemoryBarrier b{VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER};
    b.oldLayout = from;
    b.newLayout = to;
    b.srcQueueFamilyIndex = b.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    b.image = (VkImage)m.image;
    b.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    b.srcAccessMask = b.dstAccessMask = VK_ACCESS_MEMORY_READ_BIT | VK_ACCESS_MEMORY_WRITE_BIT;
    vkCmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, 0, 0, nullptr, 0, nullptr, 1, &b);
}

// Records the frame's reconstruction into `cmd`. The images are in the
// general layout before and after. 0 on success.
int lahza_ngx_evaluate(uint64_t cmd_, void* handle, const LahzaNgxFrame* f) {
    VkCommandBuffer cmd = (VkCommandBuffer)cmd_;
    const LahzaNgxImage* inputs[6] = {&f->color, &f->diffuse, &f->specular, &f->normals, &f->depth, &f->motion};
    for (const LahzaNgxImage* m : inputs) move(cmd, *m, VK_IMAGE_LAYOUT_GENERAL, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL);
    NVSDK_NGX_Resource_VK color = resource(f->color, false), diffuse = resource(f->diffuse, false), specular = resource(f->specular, false), normals = resource(f->normals, false);
    NVSDK_NGX_Resource_VK depth = resource(f->depth, false), motion = resource(f->motion, false), output = resource(f->output, true);
    NVSDK_NGX_VK_DLSSD_Eval_Params ep{};
    ep.pInColor = &color;
    ep.pInDiffuseAlbedo = &diffuse;
    ep.pInSpecularAlbedo = &specular;
    ep.pInNormals = &normals;
    ep.pInRoughness = &normals;
    ep.pInDepth = &depth;
    ep.pInMotionVectors = &motion;
    ep.pInOutput = &output;
    // (The upscaler's jitter is the image's shift, the other way from
    // where the samples went.)
    ep.InJitterOffsetX = -f->jitter[0];
    ep.InJitterOffsetY = -f->jitter[1];
    ep.InRenderSubrectDimensions = {f->width, f->height};
    ep.InReset = f->reset;
    ep.InMVScaleX = 1;
    ep.InMVScaleY = 1;
    NVSDK_NGX_Result r = NGX_VULKAN_EVALUATE_DLSSD_EXT(cmd, (NVSDK_NGX_Handle*)handle, g_params, &ep);
    for (const LahzaNgxImage* m : inputs) move(cmd, *m, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL, VK_IMAGE_LAYOUT_GENERAL);
    return NVSDK_NGX_FAILED(r) ? 1 : 0;
}

void lahza_ngx_release(void* handle) {
    if (handle) NVSDK_NGX_VULKAN_ReleaseFeature((NVSDK_NGX_Handle*)handle);
}

void lahza_ngx_shutdown(uint64_t device) {
    if (g_params) NVSDK_NGX_VULKAN_DestroyParameters(g_params);
    g_params = nullptr;
    NVSDK_NGX_VULKAN_Shutdown1((VkDevice)device);
}

}
