// ============================================================
// File: control_texture.h
// Purpose: register the Rust control-preview DXGI shared texture as a
//          Flutter external texture (GPU surface texture).
// Layer: Windows runner (host).
// Called by: flutter_window.cpp OnCreate (Attach) / OnDestroy (Detach).
// Boundary: no pixel copies here; Rust owns all pixel writes.
// ============================================================

#ifndef RUNNER_CONTROL_TEXTURE_H_
#define RUNNER_CONTROL_TEXTURE_H_

#include <flutter/flutter_engine.h>

namespace rdc_control_texture {

// Attach method channel "rdc/control_texture" on the given engine.
// `engine` must outlive this module.
void Attach(flutter::FlutterEngine* engine);

// Unregister the external texture and detach the channel.
void Detach();

}  // namespace rdc_control_texture

#endif  // RUNNER_CONTROL_TEXTURE_H_
