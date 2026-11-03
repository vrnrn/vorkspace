import Foundation
import CoreAudio

struct InputDevice: Codable, Equatable {
    let id: AudioDeviceID
    let uid: String
    let name: String
    let builtin: Bool
}
func audioScalar(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) -> UInt32? {
    var address = AudioObjectPropertyAddress(mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var value: UInt32 = 0, size = UInt32(MemoryLayout<UInt32>.size)
    return AudioObjectGetPropertyData(object, &address, 0, nil, &size, &value) == noErr ? value : nil
}
func audioString(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) -> String? {
    var address = AudioObjectPropertyAddress(mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var value: Unmanaged<CFString>?, size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
    guard AudioObjectGetPropertyData(object, &address, 0, nil, &size, &value) == noErr else { return nil }
    return value?.takeRetainedValue() as String?
}
func inputDevices() -> [InputDevice] {
    var address = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDevices, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size) == noErr else { return [] }
    guard size > 0 else { return [] }
    var ids = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
    let status = ids.withUnsafeMutableBytes { AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, $0.baseAddress!) }
    guard status == noErr else { return [] }
    return ids.compactMap { id in
        var streams = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyStreams, mScope: kAudioDevicePropertyScopeInput, mElement: kAudioObjectPropertyElementMain)
        var bytes: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(id, &streams, 0, nil, &bytes) == noErr, bytes > 0,
              audioScalar(id, kAudioDevicePropertyDeviceIsAlive) == 1,
              let uid = audioString(id, kAudioDevicePropertyDeviceUID), !uid.isEmpty else { return nil }
        return InputDevice(id: id, uid: uid, name: audioString(id, kAudioObjectPropertyName) ?? "Microphone", builtin: audioScalar(id, kAudioDevicePropertyTransportType) == kAudioDeviceTransportTypeBuiltIn)
    }.sorted { $0.uid < $1.uid }
}
func defaultInputDevice() -> AudioDeviceID? {
    audioScalar(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultInputDevice)
}
// No name matching or arbitrary first-device fallback. Explicit UIDs fail closed.
func resolveMicrophone(_ choice: String, devices: [InputDevice], system: AudioDeviceID?) -> AudioDeviceID? {
    if choice == "builtin" { return devices.first(where: { $0.builtin })?.id }
    if choice == "system" { return devices.first(where: { $0.id == system })?.id }
    guard choice.hasPrefix("uid:") else { return nil }
    return devices.first(where: { $0.uid == String(choice.dropFirst(4)) })?.id
}
// Enumeration opens no audio engine and requests no permission. Shared by Settings
// and the helper; these are CoreAudio UIDs, unrelated to WebKit's Vox device IDs.
@_cdecl("vorkspace_dictation_devices")
func dictationDevices(_ callback: @convention(c) (UnsafePointer<CChar>) -> Void) {
    let json = (try? JSONEncoder().encode(inputDevices())).flatMap { String(data: $0, encoding: .utf8) } ?? "[]"
    json.withCString { callback($0) }
}
