import Foundation

/// Cheap FNV-1a hash sampled every Nth byte (default 64) for fast change detection.
public enum PerceptualHash {
    public static func hash(_ data: Data, stride: Int = 64) -> UInt64 {
        var h: UInt64 = 0xcbf29ce484222325
        let count = data.count
        if count == 0 { return h }
        data.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
            let base = raw.bindMemory(to: UInt8.self).baseAddress!
            var i = 0
            while i < count {
                h ^= UInt64(base[i])
                h = h &* 0x100000001b3
                i += stride
            }
        }
        return h
    }
}
