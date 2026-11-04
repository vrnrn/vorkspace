import AVFoundation
import CoreMedia
import ScreenCaptureKit
import Speech

/// System audio uses the same converter and bounded analyzer input as dictation.
/// No screen output is attached and no captured media is written to disk.
final class SystemSpeechCapture: NSObject, SpeechInput, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    let stream: AsyncThrowingStream<AnalyzerInput, Error>
    private let continuation: AsyncThrowingStream<AnalyzerInput, Error>.Continuation
    private let format: AVAudioFormat
    private let origin: CMTime
    private let lock = NSLock()
    private var stopped = false
    private var converter: SpeechBufferConverter?
    private let queue = DispatchQueue(label: "vorkspace.system-speech")
    @MainActor private var capture: SCStream?
    @MainActor private var startup: Task<Void, Never>?
    @MainActor private var shutdown: Task<Void, Never>?
    var onReady: (@Sendable () -> Void)?
    var onFailure: (@Sendable (Error) -> Void)?
    var level: Float { 0 }
    init(format: AVAudioFormat, origin: CMTime) {
        (stream, continuation) = AsyncThrowingStream.makeStream(bufferingPolicy: .bufferingOldest(32))
        self.format = format; self.origin = origin
    }
    private var isStopped: Bool { lock.withLock { stopped } }
    @MainActor func start() throws {
        startup = Task {
            do {
                let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
                guard !self.isStopped, !Task.isCancelled else { return }
                guard let display = content.displays.first(where: { $0.displayID == CGMainDisplayID() }) ?? content.displays.first else {
                    throw DictationError("No audio capture display is available.")
                }
                let configuration = SCStreamConfiguration()
                configuration.capturesAudio = true
                configuration.excludesCurrentProcessAudio = true
                configuration.captureMicrophone = false
                configuration.sampleRate = 48_000
                configuration.channelCount = 2
                configuration.width = 2; configuration.height = 2
                configuration.minimumFrameInterval = CMTime(seconds: 1, preferredTimescale: 1)
                let filter = SCContentFilter(display: display, excludingWindows: [])
                let capture = SCStream(filter: filter, configuration: configuration, delegate: self)
                self.capture = capture
                try capture.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue)
                try await capture.startCapture()
                guard !self.isStopped, !Task.isCancelled else { return }
                self.onReady?()
            } catch { self.fail(error) }
        }
    }
    @MainActor func stop() {
        guard shutdown == nil else { return }
        lock.withLock { stopped = true }
        continuation.finish()
        startup?.cancel()
        let startup = startup
        shutdown = Task {
            await startup?.value
            if let capture = self.capture { try? await capture.stopCapture() }
            self.capture = nil
        }
    }
    @MainActor func close() async { stop(); await shutdown?.value }
    private func fail(_ error: Error) {
        guard !isStopped else { return }
        continuation.finish(throwing: error)
        onFailure?(error)
    }
    func stream(_ stream: SCStream, didStopWithError error: Error) { fail(error) }
    func stream(_ stream: SCStream, didOutputSampleBuffer sample: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio, !isStopped, sample.isValid else { return }
        do {
            guard let description = sample.formatDescription else { throw DictationError("System audio has no usable format.") }
            let natural = AVAudioFormat(cmAudioFormatDescription: description)
            let frames = sample.numSamples
            guard frames > 0, frames <= 8192,
                  let pcm = AVAudioPCMBuffer(pcmFormat: natural, frameCapacity: AVAudioFrameCount(frames)) else {
                throw DictationError("System audio exceeded its buffer limit.")
            }
            pcm.frameLength = AVAudioFrameCount(frames)
            let status = CMSampleBufferCopyPCMDataIntoAudioBufferList(sample, at: 0, frameCount: Int32(frames), into: pcm.mutableAudioBufferList)
            guard status == noErr else { throw DictationError("System audio could not be read.") }
            if converter == nil { converter = try SpeechBufferConverter(from: natural, to: format) }
            let (output, time) = try converter!.convert(pcm, at: sample.presentationTimeStamp - origin)
            guard output.frameLength > 0 else { return }
            guard !isStopped else { return }
            if case .dropped = continuation.yield(AnalyzerInput(buffer: output, bufferStartTime: time)) {
                throw DictationError("System audio could not keep up. The saved transcript is preserved.")
            }
        } catch { fail(error) }
    }
}
