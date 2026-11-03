import Foundation

// Kept separate from the inference bridge so Settings can check support
// without linking the Core ML engine into the main application.
@_cdecl("vorkspace_eou_supported")
func vorkspaceEouSupported() -> Int32 {
    #if arch(arm64)
    if #available(macOS 14.0, *) { return 1 }
    #endif
    return 0
}
