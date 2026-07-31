import AppKit
import Foundation

/// Shell-side watchdog (T065, FR-034): detects a hung Rust core (an FFI probe
/// that fails to answer within `hangThreshold`) and performs a
/// session-preserving restart. Session state is already snapshotted
/// continuously by the core, so relaunching loses nothing.
final class Watchdog {
    static let shared = Watchdog()

    /// Contract: a core stalled for more than 5s is considered hung.
    private let hangThreshold: TimeInterval = 5
    private let probeInterval: TimeInterval = 10
    private let queue = DispatchQueue(label: "dev.blaze.watchdog", qos: .utility)
    private var timer: DispatchSourceTimer?
    /// Consecutive misses required so one slow probe never kills the app.
    private var missedProbes = 0

    private init() {}

    func start() {
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + probeInterval, repeating: probeInterval)
        timer.setEventHandler { [weak self] in self?.probe() }
        timer.resume()
        self.timer = timer
    }

    func stop() {
        timer?.cancel()
        timer = nil
    }

    /// Round-trip a trivial FFI call through the core's locks. If the core's
    /// state mutex is deadlocked or the writer thread is wedged, this hangs.
    private func probe() {
        let done = DispatchSemaphore(value: 0)
        DispatchQueue.global(qos: .userInitiated).async {
            _ = try? CoreBridge.shared.core?.getStateJson()
            done.signal()
        }
        if done.wait(timeout: .now() + hangThreshold) == .timedOut {
            missedProbes += 1
            NSLog("Blaze watchdog: core probe timed out (%d consecutive)", missedProbes)
            if missedProbes >= 2 {
                restartPreservingSession()
            }
        } else {
            missedProbes = 0
        }
    }

    /// Relaunch the app. The core's continuous session snapshots + WAL mean
    /// the new process restores every window and tab (kill -9 durability, T040).
    private func restartPreservingSession() {
        NSLog("Blaze watchdog: core hung — restarting with session preserved")
        DispatchQueue.main.async {
            let url = Bundle.main.bundleURL
            let config = NSWorkspace.OpenConfiguration()
            config.createsNewApplicationInstance = true
            NSWorkspace.shared.openApplication(at: url, configuration: config) { _, _ in
                // Hard-exit: the hung core cannot run graceful teardown.
                exit(70) // EX_SOFTWARE
            }
        }
    }
}
