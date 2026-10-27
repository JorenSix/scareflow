// Renders the 🎃 emoji to a 256×256 RGBA buffer for the window/Dock icon.
// swift make-icon.swift  →  pumpkin.rgba (raw, straight alpha, row-major)
import AppKit

let size = 256
let space = CGColorSpaceCreateDeviceRGB()
let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: size * 4,
                    space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)
let text = NSAttributedString(string: "🎃", attributes: [.font: NSFont(name: "Apple Color Emoji", size: 200)!])
let bounds = text.size()
text.draw(at: NSPoint(x: (CGFloat(size) - bounds.width) / 2, y: (CGFloat(size) - bounds.height) / 2))
var bytes = [UInt8](UnsafeBufferPointer(start: ctx.data!.assumingMemoryBound(to: UInt8.self), count: size * size * 4))
// CoreGraphics gives premultiplied alpha; egui icons expect straight alpha.
for i in stride(from: 0, to: bytes.count, by: 4) where bytes[i + 3] > 0 {
    let a = Double(bytes[i + 3]) / 255
    for c in 0..<3 { bytes[i + c] = UInt8(min(255, (Double(bytes[i + c]) / a).rounded())) }
}
try! Data(bytes).write(to: URL(fileURLWithPath: "pumpkin.rgba"))
