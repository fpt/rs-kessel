import Foundation

/// Downloads GGUF models from HuggingFace with progress reporting.
public struct ModelDownloader {
    private static let logger = Logger("ModelDownloader")

    /// Ensure a model file exists at `path`, downloading from HuggingFace if needed.
    /// - Parameters:
    ///   - path: Local file path for the model
    ///   - repo: HuggingFace repo (e.g. "Qwen/Qwen3-8B-GGUF")
    ///   - file: Filename in the repo (e.g. "Qwen3-8B-Q4_K_M.gguf")
    /// - Returns: The resolved path (same as input)
    public static func ensureModel(path: String, repo: String, file: String) async throws -> String {
        if FileManager.default.fileExists(atPath: path) {
            logger.info("Model found at \(path)")
            return path
        }

        // Create parent directory if needed
        let dir = (path as NSString).deletingLastPathComponent
        try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)

        let url = URL(string: "https://huggingface.co/\(repo)/resolve/main/\(file)")!
        logger.info("Downloading model from \(url)")

        let tempPath = path + ".download"

        // Use delegate-based download for progress
        let delegate = DownloadDelegate(destinationPath: path, tempPath: tempPath)
        let session = URLSession(configuration: .default, delegate: delegate, delegateQueue: nil)

        var request = URLRequest(url: url)
        // Resume partial download if temp file exists
        if FileManager.default.fileExists(atPath: tempPath),
           let attrs = try? FileManager.default.attributesOfItem(atPath: tempPath),
           let size = attrs[.size] as? Int64, size > 0 {
            request.setValue("bytes=\(size)-", forHTTPHeaderField: "Range")
            logger.info("Resuming download from byte \(size)")
        }

        let task = session.downloadTask(with: request)
        task.resume()

        // Wait for completion
        try await delegate.waitForCompletion()

        logger.info("Download complete: \(path)")
        return path
    }
}

/// URLSession delegate that tracks download progress and moves file on completion.
private class DownloadDelegate: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
    let destinationPath: String
    let tempPath: String
    private let continuation: UnsafeContinuation<Void, any Error>
    private let completed: Bool = false
    private var lastPercent: Int = -1

    private class State {
        var continuation: UnsafeContinuation<Void, any Error>?
    }
    private let state = State()

    init(destinationPath: String, tempPath: String) {
        self.destinationPath = destinationPath
        self.tempPath = tempPath
        // Placeholder — real continuation set in waitForCompletion
        self.continuation = unsafeBitCast(0, to: UnsafeContinuation<Void, any Error>.self)
    }

    func waitForCompletion() async throws {
        try await withUnsafeThrowingContinuation { cont in
            self.state.continuation = cont
        }
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask,
                    didFinishDownloadingTo location: URL) {
        do {
            let fm = FileManager.default
            if fm.fileExists(atPath: destinationPath) {
                try fm.removeItem(atPath: destinationPath)
            }
            // Remove temp file if it exists
            if fm.fileExists(atPath: tempPath) {
                try fm.removeItem(atPath: tempPath)
            }
            try fm.moveItem(at: location, to: URL(fileURLWithPath: destinationPath))
            print() // newline after progress
            state.continuation?.resume()
            state.continuation = nil
        } catch {
            state.continuation?.resume(throwing: error)
            state.continuation = nil
        }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: (any Error)?) {
        if let error = error {
            print() // newline after progress
            state.continuation?.resume(throwing: error)
            state.continuation = nil
        }
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask,
                    didWriteData bytesWritten: Int64,
                    totalBytesWritten: Int64,
                    totalBytesExpectedToWrite: Int64) {
        guard totalBytesExpectedToWrite > 0 else { return }
        let percent = Int(Double(totalBytesWritten) / Double(totalBytesExpectedToWrite) * 100)
        if percent != lastPercent {
            lastPercent = percent
            let mb = Double(totalBytesWritten) / 1_000_000
            let totalMb = Double(totalBytesExpectedToWrite) / 1_000_000
            print(String(format: "\rDownloading model: %.0f/%.0f MB (%d%%)", mb, totalMb, percent), terminator: "")
            fflush(stdout)
        }
    }
}
