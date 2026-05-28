// R-ShareMouse virtual display driver scaffold.
//
// This file intentionally stops before registering an IddCx adapter. The Rust
// daemon now has the control-plane contract; the next driver phase must replace
// this scaffold with a real UMDF/IddCx implementation that reports monitor
// arrival/removal and consumes mode changes from a control path.

#include <windows.h>

extern "C" BOOL WINAPI DllMain(HINSTANCE, DWORD reason, LPVOID) {
    switch (reason) {
    case DLL_PROCESS_ATTACH:
    case DLL_PROCESS_DETACH:
    case DLL_THREAD_ATTACH:
    case DLL_THREAD_DETACH:
        break;
    default:
        break;
    }
    return TRUE;
}
