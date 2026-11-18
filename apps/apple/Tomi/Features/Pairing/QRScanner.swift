import AVFoundation
import SwiftUI
import VisionKit

/// The camera view that reads a QR code (Android `QrScanner`): VisionKit's live scanner when the device supports it,
/// otherwise an `AVCaptureSession` with a metadata output. Reports the first code read, once.
struct QRScanner: View {
    let onScanned: (String) -> Void

    /// A camera that can read codes exists on this device (never on the simulator).
    static var isAvailable: Bool {
        (DataScannerViewController.isSupported && DataScannerViewController.isAvailable) || AVCaptureDevice.default(for: .video) != nil
    }

    var body: some View {
        if DataScannerViewController.isSupported && DataScannerViewController.isAvailable {
            LiveTextScanner(onScanned: onScanned)
        } else {
            CaptureScanner(onScanned: onScanned)
        }
    }

    /// Asks for the camera if needed (only when the user touches "Scanner"); true when it may be used.
    static func requestAccess() async -> Bool {
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: true
        case .notDetermined: await AVCaptureDevice.requestAccess(for: .video)
        default: false
        }
    }
}

private struct LiveTextScanner: UIViewControllerRepresentable {
    let onScanned: (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.qr])],
            qualityLevel: .balanced,
            isHighlightingEnabled: true
        )
        scanner.delegate = context.coordinator
        try? scanner.startScanning()
        return scanner
    }

    func updateUIViewController(_ controller: DataScannerViewController, context: Context) {}

    static func dismantleUIViewController(_ controller: DataScannerViewController, coordinator: Coordinator) {
        controller.stopScanning()
    }

    func makeCoordinator() -> Coordinator { Coordinator(onScanned: onScanned) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        private var onScanned: ((String) -> Void)?

        init(onScanned: @escaping (String) -> Void) {
            self.onScanned = onScanned
        }

        func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            for case let .barcode(code) in items {
                guard let value = code.payloadStringValue, let report = onScanned else { continue }
                onScanned = nil
                scanner.stopScanning()
                report(value)
                return
            }
        }
    }
}

private struct CaptureScanner: UIViewRepresentable {
    let onScanned: (String) -> Void

    func makeUIView(context: Context) -> PreviewView {
        let view = PreviewView()
        let session = AVCaptureSession()
        if let device = AVCaptureDevice.default(for: .video), let input = try? AVCaptureDeviceInput(device: device), session.canAddInput(input) {
            session.addInput(input)
            let output = AVCaptureMetadataOutput()
            if session.canAddOutput(output) {
                session.addOutput(output)
                output.setMetadataObjectsDelegate(context.coordinator, queue: .main)
                output.metadataObjectTypes = [.qr]
            }
        }
        view.previewLayer.session = session
        view.previewLayer.videoGravity = .resizeAspectFill
        context.coordinator.session = session
        DispatchQueue.global(qos: .userInitiated).async { session.startRunning() }
        return view
    }

    func updateUIView(_ view: PreviewView, context: Context) {}

    static func dismantleUIView(_ view: PreviewView, coordinator: Coordinator) {
        let session = coordinator.session
        DispatchQueue.global(qos: .userInitiated).async { session?.stopRunning() }
    }

    func makeCoordinator() -> Coordinator { Coordinator(onScanned: onScanned) }

    final class PreviewView: UIView {
        override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }
        var previewLayer: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }
    }

    final class Coordinator: NSObject, AVCaptureMetadataOutputObjectsDelegate {
        var session: AVCaptureSession?
        private var onScanned: ((String) -> Void)?

        init(onScanned: @escaping (String) -> Void) {
            self.onScanned = onScanned
        }

        func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject], from connection: AVCaptureConnection) {
            guard let value = objects.compactMap({ ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue }).first,
                  let report = onScanned else { return }
            onScanned = nil
            report(value)
        }
    }
}
