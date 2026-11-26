import AppKit
for (index, screen) in NSScreen.screens.enumerated() {
    let space = screen.colorSpace
    let name = space?.localizedName ?? "?"
    let data = space?.iccProfileData
    let main = screen == NSScreen.main
    print("screen \(index) main=\(main) frame=\(screen.frame.size) scale=\(screen.backingScaleFactor) profile=\(name) icc_bytes=\(data?.count ?? 0) edr_max=\(screen.maximumPotentialExtendedDynamicRangeColorComponentValue)")
    if let data { try? data.write(to: URL(fileURLWithPath: "display-\(index).icc")) }
}
