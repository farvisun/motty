import CoreGraphics
// usage: warp <x> <y>  -> moves the mouse pointer to a screen point (points,
// top-left origin), so hover effects don't leak into parity captures.
let a = CommandLine.arguments
guard a.count == 3, let x = Double(a[1]), let y = Double(a[2]) else { exit(2) }
CGWarpMouseCursorPosition(CGPoint(x: x, y: y))
