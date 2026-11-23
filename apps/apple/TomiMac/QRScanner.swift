import AVFoundation
import SwiftUI
import Vision

/// The camera of the Mac reading a QR code (the iOS counterpart is `Tomi/Features/Pairing/QRScanner.swift`): the code a
/// device already in the space shows on "Ajouter un appareil", or the QR code of an installation key. Reports the
/// first code read, once. The frames of the camera are read by Vision on this Mac and never kept.
struct QRScanner: View {
    let onScanned: (String) -> Void

    /// A camera exists on this Mac (none on a Mac mini without a camera, nor with the lid closed and no display camera).
    static var isAvailable: Bool { camera != nil }

    /// The camera of the Mac itself rather than one lent by an iPhone (the phone is what shows the code).
    fileprivate static var camera: AVCaptureDevice? {
        AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .unspecified) ?? AVCaptureDevice.default(for: .video)
    }

    var body: some View {
        CameraScanner(onScanned: onScanned)
    }

    /// Asks for the camera if needed (only when the user clicks "Scanner"); true when it may be used.
    static func requestAccess() async -> Bool {
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: true
        case .notDetermined: await AVCaptureDevice.requestAccess(for: .video)
        default: false
        }
    }
}

/// Reads the QR code of an image (a frame of the camera).
enum QRDecoder {
    static func payload(in image: CGImage) -> String? {
        payload(VNImageRequestHandler(cgImage: image))
    }

    static func payload(in frame: CVPixelBuffer) -> String? {
        payload(VNImageRequestHandler(cvPixelBuffer: frame))
    }

    private static func payload(_ handler: VNImageRequestHandler) -> String? {
        let request = VNDetectBarcodesRequest()
        request.symbologies = [.qr]
        guard (try? handler.perform([request])) != nil else { return nil }
        return request.results?.compactMap(\.payloadStringValue).first
    }
}

private struct CameraScanner: NSViewRepresentable {
    let onScanned: (String) -> Void

    func makeNSView(context: Context) -> PreviewView {
        let view = PreviewView()
        let session = AVCaptureSession()
        if let device = QRScanner.camera, let input = try? AVCaptureDeviceInput(device: device), session.canAddInput(input) {
            session.addInput(input)
            let output = AVCaptureVideoDataOutput()
            output.alwaysDiscardsLateVideoFrames = true
            if session.canAddOutput(output) {
                session.addOutput(output)
                output.setSampleBufferDelegate(context.coordinator, queue: context.coordinator.queue)
            }
        }
        view.previewLayer.session = session
        view.previewLayer.videoGravity = .resizeAspectFill
        context.coordinator.start(session)
        return view
    }

    func updateNSView(_ view: PreviewView, context: Context) {}

    static func dismantleNSView(_ view: PreviewView, coordinator: Coordinator) {
        coordinator.stop()
    }

    func makeCoordinator() -> Coordinator { Coordinator(onScanned: onScanned) }

    final class PreviewView: NSView {
        let previewLayer = AVCaptureVideoPreviewLayer()

        override init(frame: NSRect) {
            super.init(frame: frame)
            wantsLayer = true
            layer = previewLayer
        }

        @available(*, unavailable)
        required init?(coder: NSCoder) { fatalError("not used") }
    }

    /// Reads one frame out of a few (a QR code held in front of the camera stays there), off the main thread.
    final class Coordinator: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate, @unchecked Sendable {
        let queue = DispatchQueue(label: "eu.shaffe.tomi.qr-scan", qos: .userInitiated)
        /// Read and written on `queue` only.
        private var session: AVCaptureSession?
        private let onScanned: (String) -> Void
        /// Read and written on `queue` only.
        private var done = false
        private var frames = 0

        init(onScanned: @escaping (String) -> Void) {
            self.onScanned = onScanned
        }

        /// Starts the camera. On `queue`, as every stop: a view removed at once stops after its start, never before
        /// (the camera would stay on).
        func start(_ session: AVCaptureSession) {
            queue.async { [weak self] in
                guard let self, !self.done else { return }
                self.session = session
                session.startRunning()
            }
        }

        func stop() {
            queue.async { [weak self] in
                self?.done = true
                self?.session?.stopRunning()
            }
        }

        func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
            guard !done else { return }
            frames += 1
            guard frames % 4 == 0, let frame = CMSampleBufferGetImageBuffer(sampleBuffer),
                  let value = QRDecoder.payload(in: frame) else { return }
            done = true
            // Not from inside the delegate of the frames: stopping waits for it.
            queue.async { [weak self] in self?.session?.stopRunning() }
            let report = onScanned
            DispatchQueue.main.async { report(value) }
        }
    }
}
