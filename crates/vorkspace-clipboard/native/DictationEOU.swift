// The clipboard helper owns this handle on its model worker thread. Audio is
// already 16 kHz mono PCM; the FluidAudio engine never opens a microphone.
import CoreML
import Darwin
import Foundation

private final class EouHandle {
    let engine: StreamingEouAsrManager
    init(_ engine: StreamingEouAsrManager) { self.engine = engine }
}

private func eouError(_ destination: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?, _ message: String) {
    destination?.pointee = strdup(message)
}

private func eouHandle(_ pointer: UnsafeMutableRawPointer?) -> EouHandle? {
    guard let pointer else { return nil }
    return Unmanaged<EouHandle>.fromOpaque(pointer).takeUnretainedValue()
}

@_cdecl("vorkspace_eou_load")
func vorkspaceEouLoad(_ path: UnsafePointer<CChar>?, _ failure: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> UnsafeMutableRawPointer? {
    guard #available(macOS 14.0, *), vorkspaceEouSupported() == 1, let path else {
        eouError(failure, "Parakeet Flash requires Apple silicon and macOS 14 or newer.")
        return nil
    }
    do {
        let configuration = MLModelConfiguration()
        configuration.computeUnits = .cpuAndNeuralEngine
        let engine = StreamingEouAsrManager(configuration: configuration, chunkSize: .ms160)
        try engine.loadModels(modelDir: URL(fileURLWithPath: String(cString: path), isDirectory: true))
        return Unmanaged.passRetained(EouHandle(engine)).toOpaque()
    } catch {
        eouError(failure, "Could not prepare Parakeet Flash: \(error.localizedDescription)")
        return nil
    }
}

@_cdecl("vorkspace_eou_reset")
func vorkspaceEouReset(_ pointer: UnsafeMutableRawPointer?, _ failure: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32 {
    guard let handle = eouHandle(pointer) else { eouError(failure, "Parakeet Flash is unavailable."); return 1 }
    do { try handle.engine.reset(); return 0 }
    catch { eouError(failure, "Could not reset Parakeet Flash: \(error.localizedDescription)"); return 1 }
}

@_cdecl("vorkspace_eou_feed")
func vorkspaceEouFeed(_ pointer: UnsafeMutableRawPointer?, _ samples: UnsafePointer<Float>?, _ count: Int32, _ failure: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> UnsafeMutablePointer<CChar>? {
    guard let handle = eouHandle(pointer), let samples, count >= 0, count <= 16000 else {
        eouError(failure, "Invalid Parakeet Flash audio block."); return nil
    }
    do {
        handle.engine.appendSamples(Array(UnsafeBufferPointer(start: samples, count: Int(count))))
        try handle.engine.processBufferedAudio()
        return strdup(handle.engine.getPartialTranscript())
    } catch {
        eouError(failure, "Parakeet Flash transcription failed: \(error.localizedDescription)")
        return nil
    }
}

@_cdecl("vorkspace_eou_finish")
func vorkspaceEouFinish(_ pointer: UnsafeMutableRawPointer?, _ failure: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> UnsafeMutablePointer<CChar>? {
    guard let handle = eouHandle(pointer) else { eouError(failure, "Parakeet Flash is unavailable."); return nil }
    do { return strdup(try handle.engine.finish()) }
    catch { eouError(failure, "Parakeet Flash transcription failed: \(error.localizedDescription)"); return nil }
}

@_cdecl("vorkspace_eou_free")
func vorkspaceEouFree(_ pointer: UnsafeMutableRawPointer?) {
    guard let pointer else { return }
    Unmanaged<EouHandle>.fromOpaque(pointer).release()
}
