import AVFoundation
import AudioToolbox
import CoreAudio
import Speech

protocol SpeechInput: AnyObject {
    var stream: AsyncThrowingStream<AnalyzerInput, Error> { get }
    var level: Float { get }
    var onReady: (@Sendable () -> Void)? { get set }
    var onFailure: (@Sendable (Error) -> Void)? { get set }
    @MainActor func start() throws
    @MainActor func stop()
    @MainActor func check() throws
}
extension SpeechInput { @MainActor func check() throws {} }

/// Shared by microphone capture and the explicit, file-only speech fixture.
final class SpeechBufferConverter {
    let natural: AVAudioFormat
    let format: AVAudioFormat
    private let converter: AVAudioConverter
    init(from natural: AVAudioFormat, to format: AVAudioFormat) throws {
        guard natural.sampleRate > 0, natural.channelCount > 0,
              let converter = AVAudioConverter(from: natural, to: format) else {
            throw DictationError("This microphone has no usable audio format. Choose another microphone.")
        }
        self.natural = natural; self.format = format; self.converter = converter
        converter.primeMethod = .none
    }
    func convert(_ buffer: AVAudioPCMBuffer) throws -> AVAudioPCMBuffer {
        guard buffer.format == natural, buffer.frameLength > 0, buffer.frameLength <= 8192 else {
            throw DictationError("The microphone format changed. Try again.")
        }
        let frames = ceil(Double(buffer.frameLength) * format.sampleRate / natural.sampleRate) + 32
        guard frames <= 16_384, let output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(frames)) else {
            throw DictationError("The microphone produced an unsupported audio buffer.")
        }
        var supplied = false
        var error: NSError?
        let status = converter.convert(to: output, error: &error) { _, status in
            if supplied { status.pointee = .noDataNow; return nil }
            supplied = true; status.pointee = .haveData; return buffer
        }
        guard status != .error, error == nil else {
            throw DictationError("Microphone audio could not be converted. Try again.")
        }
        return output
    }
}

/// The tap owns conversion; the analyzer receives owned, bounded PCM buffers.
/// There is no audio file, idle microphone, or second transcription queue.
final class SpeechCapture: SpeechInput, @unchecked Sendable {
    let stream: AsyncThrowingStream<AnalyzerInput, Error>
    private let continuation: AsyncThrowingStream<AnalyzerInput, Error>.Continuation
    private let engine = AVAudioEngine()
    private let lock = NSLock()
    private let device: AudioDeviceID
    private let natural: AVAudioFormat
    private let converter: SpeechBufferConverter
    private var observer: NSObjectProtocol?
    private var tapped = false
    private var stopped = false
    private var heardInput = false
    private var startup: Task<Void, Never>?
    private var amplitude: Float = 0
    var onReady: (@Sendable () -> Void)?
    var onFailure: (@Sendable (Error) -> Void)?

    init(microphone: String, format: AVAudioFormat) throws {
        (stream, continuation) = AsyncThrowingStream.makeStream(bufferingPolicy: .bufferingOldest(32))
        guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
            throw DictationError("Allow microphone access for vorkspace Clipboard in Dictation settings.")
        }
        guard let device = resolveMicrophone(microphone, devices: inputDevices(), system: defaultInputDevice()) else {
            throw DictationError("The selected microphone is unavailable. Connect it or choose another microphone.")
        }
        self.device = device
        let input = engine.inputNode
        try input.auAudioUnit.setDeviceID(device)
        natural = input.outputFormat(forBus: 0)
        converter = try SpeechBufferConverter(from: natural, to: format)
    }

    @MainActor
    func start() throws {
        guard routeIsValid() else { throw DictationError("The microphone changed before recording started. Try again.") }
        observer = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main) { [weak self] _ in
            guard let self, !self.isStopped else { return }
            if !self.engine.isRunning || !self.routeIsValid() {
                self.onFailure?(DictationError("The microphone disconnected or its audio route changed. Try again."))
            }
        }
        engine.inputNode.installTap(onBus: 0, bufferSize: 2048, format: natural) { [weak self] buffer, _ in
            self?.receive(buffer)
        }
        tapped = true
        engine.prepare()
        try engine.start()
        startup = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(2))
            guard !Task.isCancelled, let self, !self.isStopped, !self.hasInput else { return }
            self.onFailure?(DictationError("The microphone did not deliver audio. Check its connection and try again."))
        }
    }

    private func routeIsValid() -> Bool {
        guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized,
              audioScalar(device, kAudioDevicePropertyDeviceIsAlive) == 1,
              let unit = engine.inputNode.audioUnit,
              engine.inputNode.outputFormat(forBus: 0) == natural else { return false }
        var actual: AudioDeviceID = 0
        var bytes = UInt32(MemoryLayout<AudioDeviceID>.size)
        return AudioUnitGetProperty(unit, kAudioOutputUnitProperty_CurrentDevice,
            kAudioUnitScope_Global, 0, &actual, &bytes) == noErr && actual == device
    }

    @MainActor func check() throws {
        guard routeIsValid(), !tapped || engine.isRunning else {
            throw DictationError("The microphone disconnected or its audio route changed. Try again.")
        }
    }

    private var isStopped: Bool { lock.withLock { stopped } }
    private var hasInput: Bool { lock.withLock { heardInput } }
    var level: Float { lock.withLock { amplitude } }

    private func receive(_ buffer: AVAudioPCMBuffer) {
        lock.lock()
        defer { lock.unlock() }
        guard !stopped else { return }
        let output: AVAudioPCMBuffer
        do { output = try converter.convert(buffer) }
        catch { continuation.finish(throwing: error); onFailure?(error); return }
        guard output.frameLength > 0 else { return }
        if let samples = output.floatChannelData?[0] {
            var sum: Float = 0
            for index in 0..<Int(output.frameLength) { sum += samples[index] * samples[index] }
            amplitude = min(1, sqrt(sum / Float(output.frameLength)) * 12)
        }
        switch continuation.yield(AnalyzerInput(buffer: output)) {
        case .dropped:
            let error = DictationError("Speech recognition could not keep up with this microphone. Try again.")
            continuation.finish(throwing: error)
            onFailure?(error)
        case .enqueued:
            if !heardInput { heardInput = true; onReady?() }
        case .terminated: break
        @unknown default: continuation.finish(throwing: DictationError("Microphone input ended unexpectedly."))
        }
    }

    @MainActor
    func stop() {
        startup?.cancel(); startup = nil
        lock.withLock { stopped = true; amplitude = 0 }
        engine.stop()
        if tapped { engine.inputNode.removeTap(onBus: 0); tapped = false }
        if let observer { NotificationCenter.default.removeObserver(observer); self.observer = nil }
        continuation.finish()
    }
}
