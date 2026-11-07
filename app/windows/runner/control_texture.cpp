// ============================================================
// File: control_texture.cpp
// Purpose: bridge Rust DXGI shared textures into Flutter GPU surface
//          textures for the remote-control page.
// Layer: Windows runner (host).
// Called by: flutter_window.cpp (Attach/Detach) and the Dart
//            MethodChannel "rdc/control_texture".
// Boundary: this file performs no CPU pixel copies; Rust writes the
//           shared textures and signals a Win32 event per new frame.
// ============================================================

#include "control_texture.h"

#include <flutter/method_channel.h>
#include <flutter/standard_method_codec.h>
#include <flutter_plugin_registrar.h>
#include <flutter_texture_registrar.h>
#include <flutter_windows.h>

#include <windows.h>

#include <atomic>
#include <cstdint>
#include <memory>
#include <string>
#include <thread>

namespace rdc_control_texture {
namespace {

// Single-instance bridge state.
struct BridgeState {
  FlutterDesktopTextureRegistrarRef registrar = nullptr;  // borrowed
  int64_t texture_id = -1;                               // -1 = not registered
  HANDLE handles[2] = {nullptr, nullptr};                // double buffered
  HANDLE events[2] = {nullptr, nullptr};                 // new-frame signals
  uint32_t width = 0;
  uint32_t height = 0;
  std::atomic<int> current{0};
  std::atomic<bool> running{false};
  std::thread waiter;
};

BridgeState& state() {
  static BridgeState instance;
  return instance;
}

// Called by the Flutter render thread: return the most recently written buffer.
const FlutterDesktopGpuSurfaceDescriptor* ObtainTexture(size_t width,
                                                        size_t height,
                                                        void* user_data) {
  auto* bridge = static_cast<BridgeState*>(user_data);
  thread_local FlutterDesktopGpuSurfaceDescriptor descriptor{};
  int index = bridge->current.load(std::memory_order_acquire);
  if (index < 0 || index > 1) {
    index = 0;
  }
  descriptor.struct_size = sizeof(descriptor);
  descriptor.handle = reinterpret_cast<void*>(bridge->handles[index]);
  descriptor.width = bridge->width;
  descriptor.height = bridge->height;
  descriptor.visible_width = bridge->width;
  descriptor.visible_height = bridge->height;
  descriptor.format = kFlutterDesktopPixelFormatBGRA8888;
  descriptor.release_callback = nullptr;
  descriptor.release_context = nullptr;
  return &descriptor;
}

// Stop the event waiter thread (idempotent).
void StopWaiter() {
  auto& bridge = state();
  bridge.running.store(false, std::memory_order_release);
  if (bridge.waiter.joinable()) {
    bridge.waiter.join();
  }
}

// Unregister the current external texture (idempotent).
void UnregisterTexture() {
  auto& bridge = state();
  if (bridge.texture_id >= 0 && bridge.registrar != nullptr) {
    FlutterDesktopTextureRegistrarUnregisterExternalTexture(
        bridge.registrar, bridge.texture_id, nullptr, nullptr);
  }
  bridge.texture_id = -1;
  bridge.handles[0] = nullptr;
  bridge.handles[1] = nullptr;
  bridge.events[0] = nullptr;
  bridge.events[1] = nullptr;
}

int64_t ReadInt(const flutter::EncodableMap& args, const char* key,
                int64_t fallback) {
  auto it = args.find(flutter::EncodableValue(std::string(key)));
  if (it == args.end()) {
    return fallback;
  }
  if (const auto* value = std::get_if<int64_t>(&it->second)) {
    return *value;
  }
  if (const auto* value = std::get_if<int32_t>(&it->second)) {
    return static_cast<int64_t>(*value);
  }
  return fallback;
}

void HandleMethodCall(
    const flutter::MethodCall<flutter::EncodableValue>& call,
    std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result) {
  auto& bridge = state();
  if (call.method_name() == "register") {
    const auto* args = std::get_if<flutter::EncodableMap>(call.arguments());
    if (args == nullptr) {
      result->Error("bad_args", "register expects a map");
      return;
    }
    StopWaiter();
    UnregisterTexture();

    int64_t handle_a = ReadInt(*args, "handle_a", 0);
    int64_t handle_b = ReadInt(*args, "handle_b", 0);
    int64_t event_a = ReadInt(*args, "event_a", 0);
    int64_t event_b = ReadInt(*args, "event_b", 0);
    uint32_t width = static_cast<uint32_t>(ReadInt(*args, "width", 0));
    uint32_t height = static_cast<uint32_t>(ReadInt(*args, "height", 0));
    if (handle_a == 0 || handle_b == 0 || event_a == 0 || event_b == 0 ||
        width == 0 || height == 0) {
      result->Error("bad_args", "missing handle/event/size");
      return;
    }

    bridge.handles[0] = reinterpret_cast<HANDLE>(handle_a);
    bridge.handles[1] = reinterpret_cast<HANDLE>(handle_b);
    bridge.events[0] = reinterpret_cast<HANDLE>(event_a);
    bridge.events[1] = reinterpret_cast<HANDLE>(event_b);
    bridge.width = width;
    bridge.height = height;
    bridge.current.store(0, std::memory_order_release);

    FlutterDesktopTextureInfo info = {};
    info.type = kFlutterDesktopGpuSurfaceTexture;
    info.gpu_surface_config.struct_size =
        sizeof(FlutterDesktopGpuSurfaceTextureConfig);
    info.gpu_surface_config.type = kFlutterDesktopGpuSurfaceTypeDxgiSharedHandle;
    info.gpu_surface_config.callback = &ObtainTexture;
    info.gpu_surface_config.user_data = &bridge;

    bridge.texture_id =
        FlutterDesktopTextureRegistrarRegisterExternalTexture(bridge.registrar,
                                                              &info);
    if (bridge.texture_id < 0) {
      result->Error("register_failed", "Flutter rejected the texture");
      return;
    }

    bridge.running.store(true, std::memory_order_release);
    HANDLE events[2] = {bridge.events[0], bridge.events[1]};
    int64_t texture_id = bridge.texture_id;
    FlutterDesktopTextureRegistrarRef registrar = bridge.registrar;
    bridge.waiter = std::thread([&bridge, events, texture_id, registrar]() {
      while (bridge.running.load(std::memory_order_acquire)) {
        DWORD wait = WaitForMultipleObjects(2, events, FALSE, 100);
        if (wait == WAIT_OBJECT_0 || wait == WAIT_OBJECT_0 + 1) {
          int index = static_cast<int>(wait - WAIT_OBJECT_0);
          bridge.current.store(index, std::memory_order_release);
          FlutterDesktopTextureRegistrarMarkExternalTextureFrameAvailable(
              registrar, texture_id);
        }
      }
    });

    flutter::EncodableMap reply;
    reply[flutter::EncodableValue("texture_id")] =
        flutter::EncodableValue(texture_id);
    result->Success(flutter::EncodableValue(reply));
    return;
  }

  if (call.method_name() == "unregister") {
    StopWaiter();
    UnregisterTexture();
    result->Success();
    return;
  }

  if (call.method_name() == "status") {
    flutter::EncodableMap reply;
    reply[flutter::EncodableValue("registered")] =
        flutter::EncodableValue(bridge.texture_id >= 0);
    reply[flutter::EncodableValue("texture_id")] =
        flutter::EncodableValue(static_cast<int64_t>(bridge.texture_id));
    result->Success(flutter::EncodableValue(reply));
    return;
  }

  result->NotImplemented();
}

std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> g_channel;

}  // namespace

void Attach(flutter::FlutterEngine* engine) {
  if (engine == nullptr) {
    return;
  }
  auto& bridge = state();
  auto plugin_registrar = engine->GetRegistrarForPlugin("rdc_control_texture");
  bridge.registrar =
      FlutterDesktopRegistrarGetTextureRegistrar(plugin_registrar);
  g_channel = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      engine->messenger(), "rdc/control_texture",
      &flutter::StandardMethodCodec::GetInstance());
  g_channel->SetMethodCallHandler(
      [](const flutter::MethodCall<flutter::EncodableValue>& call,
         std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
             result) { HandleMethodCall(call, std::move(result)); });
}

void Detach() {
  StopWaiter();
  UnregisterTexture();
  g_channel.reset();
}

}  // namespace rdc_control_texture
