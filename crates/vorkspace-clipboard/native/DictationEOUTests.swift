import Foundation

@main
struct DictationEOUTests {
    static func main() {
        assert(isEouBoundaryToken(1024))
        assert(isEouBoundaryToken(1025))
        assert(!isEouBoundaryToken(1023))
        assert(!isEouBoundaryToken(1026))
        assert(StreamingChunkSize.ms160.shiftSamples == 1280)

        let mel = AudioMelSpectrogram()
        let (values, _, frames) = mel.computeFlat(audio: [Float](repeating: 0, count: 2560))
        assert(frames == 17)
        assert(values.count == 128 * frames)
        assert(values.allSatisfy(\.isFinite))

        if vorkspaceEouSupported() == 1 {
            var error: UnsafeMutablePointer<CChar>?
            let missing = NSTemporaryDirectory() + "vorkspace-missing-eou-" + UUID().uuidString
            let model = missing.withCString {
                vorkspaceEouLoad($0, &error)
            }
            assert(model == nil)
            assert(error != nil)
            free(error)
        }
        print("EOU boundaries, synthetic mel and missing-model failure passed")
    }
}
