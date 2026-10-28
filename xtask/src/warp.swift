import CoreGraphics
// usage: warp <x> <y>  -> moves the mouse pointer to a screen point (points,
// top-left origin), so hover effects don't leak into parity captures.
// A warp alone sends no events, so apps keep their old hover state; a
// mouse-moved event at the new point lets their tracking areas catch up
// (dropped silently when the process may not post events).
let a = CommandLine.arguments
guard a.count == 3, let x = Double(a[1]), let y = Double(a[2]) else { exit(2) }
let point = CGPoint(x: x, y: y)
CGWarpMouseCursorPosition(point)
if let moved = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: point, mouseButton: .left) {
    moved.post(tap: .cghidEventTap)
}
