import Foundation

public struct LinearResampler {
    public let sourceRate: Double
    public let targetRate: Double
    private var fractional: Double = 0

    public init(sourceRate: Double, targetRate: Double = 16000) {
        self.sourceRate = sourceRate
        self.targetRate = targetRate
    }

    public mutating func resample(_ input: [Float]) -> [Float] {
        guard sourceRate != targetRate else { return input }
        let ratio = sourceRate / targetRate
        var out: [Float] = []
        out.reserveCapacity(Int(Double(input.count) / ratio) + 1)
        var pos = fractional
        while pos < Double(input.count - 1) {
            let i = Int(pos)
            let frac = pos - Double(i)
            let a = Double(input[i])
            let b = Double(input[i + 1])
            out.append(Float(a + (b - a) * frac))
            pos += ratio
        }
        fractional = pos - Double(input.count)
        if fractional < 0 { fractional = 0 }
        return out
    }
}

public func pcm16Encode(_ samples: [Float]) -> Data {
    var out = Data(count: samples.count * 2)
    out.withUnsafeMutableBytes { (raw: UnsafeMutableRawBufferPointer) in
        let p = raw.baseAddress!.assumingMemoryBound(to: Int16.self)
        for i in 0..<samples.count {
            let clamped = max(-1.0, min(1.0, samples[i]))
            p[i] = Int16(clamped * 32767.0)
        }
    }
    return out
}
