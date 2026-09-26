import AppKit
import CoreLocation

@MainActor
final class SettingsLocationAccess: NSObject, CLLocationManagerDelegate {
  private let manager = CLLocationManager()
  private var completion: ((Result<Void, SettingsFailure>) -> Void)?
  override init() {
    super.init()
    manager.delegate = self
  }
  func request(_ completion: @escaping (Result<Void, SettingsFailure>) -> Void) {
    guard self.completion == nil else { return }
    self.completion = completion
    resolve(request: true)
  }
  nonisolated func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
    Task { @MainActor [weak self] in self?.resolve(request: false) }
  }
  private func resolve(request: Bool) {
    guard let completion else { return }
    switch manager.authorizationStatus {
    case .authorizedAlways, .authorizedWhenInUse:
      self.completion = nil; completion(.success(()))
    case .denied, .restricted:
      self.completion = nil
      completion(.failure(SettingsFailure(message: "Location access is denied. Allow Crawler in macOS Privacy & Security → Location Services to scan Wi-Fi networks.")))
    case .notDetermined:
      if request { manager.requestWhenInUseAuthorization() }
    @unknown default:
      self.completion = nil
      completion(.failure(SettingsFailure(message: "Location authorization is unavailable.")))
    }
  }
}
