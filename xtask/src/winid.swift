import CoreGraphics
import Foundation
// usage: winid <owner> [title-substring]  -> prints CGWindowID of the frontmost matching window
let args = CommandLine.arguments
let owner = args.count > 1 ? args[1] : "Motty"
let title = args.count > 2 ? args[2] : nil
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
for w in list where (w[kCGWindowOwnerName as String] as? String) == owner && (w[kCGWindowLayer as String] as? Int) == 0 {
    if let t = title, !((w[kCGWindowName as String] as? String) ?? "").contains(t) { continue }
    print(w[kCGWindowNumber as String] as! Int); exit(0)
}
exit(1)
