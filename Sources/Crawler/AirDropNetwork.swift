import Foundation
import Network
import Security
import os
import UniformTypeIdentifiers

struct AirDropEndpoint: Sendable {
  let host: String
  let port: UInt16
}

struct AirDropDiscoveryResponse: Decodable {
  let name: String
  enum CodingKeys: String, CodingKey { case name = "ReceiverComputerName" }
}

enum AirDropNetworkError: LocalizedError {
  case message(String)
  var errorDescription: String? { switch self { case .message(let text): text } }
}

/// AirDrop's legacy HTTPS protocol over AWDL. Trust is restricted to Apple roots;
/// these certificates identify AirDrop peers, not ordinary DNS web servers.
actor AirDropConnection {
  private let connection: NWConnection
  private let log = Logger(subsystem: "studio.crawler.native-prototype", category: "AirDrop")
  private var bufferedResponse = Data()
  init(_ endpoint: AirDropEndpoint, certificate: AirDropSenderCertificate) throws {
    guard #available(macOS 15, *) else { throw AirDropNetworkError.message("Custom AirDrop requires macOS 15 or later.") }
    var imported: CFArray?
    let status = SecPKCS12Import(certificate.data as CFData,
      [kSecImportExportPassphrase: certificate.passphrase, kSecImportToMemoryOnly: true] as CFDictionary, &imported)
    guard status == errSecSuccess, let items = imported as? [[String: Any]],
      let value = items.first?[kSecImportItemIdentity as String], CFGetTypeID(value as CFTypeRef) == SecIdentityGetTypeID() else {
      throw AirDropNetworkError.message("Couldn't load Crawler's AirDrop sender certificate (\(status)).")
    }
    // SecPKCS12Import returns CF values; validate its type at this boundary.
    let identity = value as! SecIdentity
    let tls = NWProtocolTLS.Options()
    guard let localIdentity = sec_identity_create(identity) else {
      throw AirDropNetworkError.message("Couldn't configure Crawler's AirDrop identity.")
    }
    sec_protocol_options_set_local_identity(tls.securityProtocolOptions, localIdentity)
    var systemAnchors: CFArray?
    guard SecTrustCopyAnchorCertificates(&systemAnchors) == errSecSuccess,
      let anchors = systemAnchors as? [SecCertificate] else {
      throw AirDropNetworkError.message("Couldn't load trusted Apple certificates.")
    }
    let appleRoots = anchors.filter { (SecCertificateCopySubjectSummary($0) as String?)?.hasPrefix("Apple Root CA") == true }
    guard !appleRoots.isEmpty else { throw AirDropNetworkError.message("Apple root certificates are unavailable.") }
    sec_protocol_options_set_verify_block(tls.securityProtocolOptions, { _, trust, complete in
      let value = sec_trust_copy_ref(trust).takeRetainedValue()
      SecTrustSetPolicies(value, SecPolicyCreateBasicX509())
      SecTrustSetAnchorCertificates(value, appleRoots as CFArray)
      SecTrustSetAnchorCertificatesOnly(value, true)
      complete(SecTrustEvaluateWithError(value, nil))
    }, .global(qos: .userInitiated))
    let parameters = NWParameters(tls: tls, tcp: NWProtocolTCP.Options())
    parameters.includePeerToPeer = true
    guard let port = NWEndpoint.Port(rawValue: endpoint.port) else {
      throw AirDropNetworkError.message("The device advertised an invalid port.")
    }
    connection = NWConnection(host: NWEndpoint.Host(endpoint.host), port: port, using: parameters)
  }
  nonisolated func close() { connection.cancel() }
  func connect() async throws {
    let connection = connection
    try await withTaskCancellationHandler {
      try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
        let completed = OSAllocatedUnfairLock(initialState: false)
        connection.stateUpdateHandler = { state in
          let result: Result<Void, Error>
          switch state {
          case .ready: result = .success(())
          case .failed(let error), .waiting(let error): result = .failure(error)
          case .cancelled: result = .failure(CancellationError())
          default: return
          }
          let first = completed.withLock { done in if done { return false }; done = true; return true }
          if first { continuation.resume(with: result) }
        }
        connection.start(queue: .global(qos: .userInitiated))
        DispatchQueue.global().asyncAfter(deadline: .now() + 8) {
          let timedOut = completed.withLock { done in if done { return false }; done = true; return true }
          if timedOut {
            connection.cancel()
            continuation.resume(throwing: AirDropNetworkError.message("The device didn't respond. Keep it unlocked and discoverable."))
          }
        }
      }
    } onCancel: { connection.cancel() }
  }
  private func send(_ data: Data) async throws {
    try Task.checkCancellation()
    try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
      connection.send(content: data, completion: .contentProcessed { error in
        if let error { continuation.resume(throwing: error) } else { continuation.resume() }
      })
    }
  }
  private func receive() async throws -> Data {
    try Task.checkCancellation()
    return try await withCheckedThrowingContinuation { continuation in
      connection.receive(minimumIncompleteLength: 1, maximumLength: 65_536) { data, _, complete, error in
        if let error { continuation.resume(throwing: error) }
        else if let data, !data.isEmpty { continuation.resume(returning: data) }
        else { continuation.resume(throwing: AirDropNetworkError.message(complete ? "The device closed the connection." : "The device returned no data.")) }
      }
    }
  }
  func post(_ path: String, body: Data) async throws -> Data {
    let started = ContinuousClock.now
    let deadline = deadline(after: path == "/Ask" ? 120 : 12)
    defer { deadline.cancel() }
    try await sendHeader(path, type: "application/octet-stream", count: body.count)
    try await send(body)
    let result = try await response()
    log.info("AirDrop \(path, privacy: .public) acknowledged in \(started.duration(to: .now).description, privacy: .public)")
    return result
  }
  private func deadline(after seconds: UInt64) -> Task<Void, Never> {
    let connection = connection
    return Task {
      do { try await Task.sleep(nanoseconds: seconds * 1_000_000_000) }
      catch { return }
      connection.cancel()
    }
  }
  private func sendHeader(_ path: String, type: String, count: Int?, transfer: (id: UUID, bytes: Int)? = nil) async throws {
    let length = count.map { "Content-Length: \($0)" } ?? "Transfer-Encoding: chunked"
    let transferHeaders = transfer.map { "TransferID: \($0.id.uuidString)\r\nTotalBytes: \($0.bytes)\r\n" } ?? ""
    try await send(Data("POST \(path) HTTP/1.1\r\nHost: localhost\r\nUser-Agent: AirDrop/1.0\r\nAccept: */*\r\nContent-Type: \(type)\r\n\(length)\r\n\(transferHeaders)Connection: keep-alive\r\n\r\n".utf8))
  }
  func upload(_ prepared: AirDropPreparedFiles, progress: @Sendable (Double) async -> Void) async throws {
    let started = ContinuousClock.now
    let archive = prepared.archive
    let size = try archive.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
    let file = try FileHandle(forReadingFrom: archive)
    defer { try? file.close() }
    // Current receivers bind this upload to the ID announced in the accepted /Ask.
    try await sendHeader("/Upload", type: "application/x-cpio", count: nil, transfer: (prepared.transferID, size))
    var sent = 0
    while let chunk = try file.read(upToCount: 262_144), !chunk.isEmpty {
      var frame = Data("\(String(chunk.count, radix: 16))\r\n".utf8)
      frame.append(chunk); frame.append(contentsOf: [13, 10])
      try await send(frame)
      sent += chunk.count
      await progress(Double(sent) / Double(max(1, size)))
    }
    try await send(Data("0\r\n\r\n".utf8))
    log.info("AirDrop upload submitted \(sent) archive bytes in \(started.duration(to: .now).description, privacy: .public); awaiting receiver acknowledgment")
    let deadline = deadline(after: 30)
    defer { deadline.cancel() }
    _ = try await response()
    log.info("AirDrop upload acknowledged in \(started.duration(to: .now).description, privacy: .public)")
  }
  private func response() async throws -> Data {
    var bytes = bufferedResponse
    bufferedResponse = Data()
    let delimiter = Data("\r\n\r\n".utf8)
    while bytes.range(of: delimiter) == nil {
      guard bytes.count < 32_768 else { throw AirDropNetworkError.message("Invalid AirDrop response headers.") }
      bytes.append(try await receive())
    }
    guard let split = bytes.range(of: delimiter), let header = String(data: bytes[..<split.lowerBound], encoding: .utf8) else {
      throw AirDropNetworkError.message("Invalid AirDrop response.")
    }
    let lines = header.components(separatedBy: "\r\n")
    let status = lines[0].split(separator: " ").dropFirst().first.flatMap { Int($0) }
    guard status == 200 else {
      throw AirDropNetworkError.message(status == 403 ? "The receiving device declined the request." : "The receiving device returned HTTP \(status ?? 0).")
    }
    var body = Data(bytes[split.upperBound...])
    let chunked = lines.contains { $0.lowercased().hasPrefix("transfer-encoding:") && $0.lowercased().contains("chunked") }
    if chunked {
      var output = Data()
      while true {
        while body.range(of: Data("\r\n".utf8)) == nil { body.append(try await receive()); if body.count > 1_048_576 { throw AirDropNetworkError.message("AirDrop response is too large.") } }
        guard let lineEnd = body.range(of: Data("\r\n".utf8)), let line = String(data: body[..<lineEnd.lowerBound], encoding: .ascii),
          let size = Int(line.split(separator: ";", maxSplits: 1).first ?? "", radix: 16), size >= 0, size <= 1_048_576 - output.count else {
          throw AirDropNetworkError.message("Invalid AirDrop response chunk.")
        }
        body = Data(body[lineEnd.upperBound...])
        if size == 0 {
          // Consume the final CRLF or bounded trailers before the next request.
          while !body.starts(with: [13, 10]) && body.range(of: delimiter) == nil {
            guard body.count < 32_768 else { throw AirDropNetworkError.message("Invalid AirDrop response trailers.") }
            body.append(try await receive())
          }
          if body.starts(with: [13, 10]) { bufferedResponse = Data(body.dropFirst(2)) }
          else if let end = body.range(of: delimiter) { bufferedResponse = Data(body[end.upperBound...]) }
          return output
        }
        while body.count < size + 2 { body.append(try await receive()) }
        guard body[size] == 13, body[size + 1] == 10 else { throw AirDropNetworkError.message("Invalid AirDrop response chunk.") }
        output.append(body.prefix(size)); body = Data(body.dropFirst(size + 2))
      }
    }
    guard let lengthLine = lines.first(where: { $0.lowercased().hasPrefix("content-length:") }),
      let count = Int(lengthLine.dropFirst("content-length:".count).trimmingCharacters(in: .whitespaces)), count >= 0, count <= 1_048_576 else {
      throw AirDropNetworkError.message("AirDrop response has no bounded body length.")
    }
    while body.count < count { body.append(try await receive()) }
    bufferedResponse = Data(body.dropFirst(count))
    return Data(body.prefix(count))
  }
}

struct AirDropPreparedFiles: Sendable {
  let directory: URL
  let archive: URL
  let ask: Data
  let transferID: UUID
  static func prepare(_ files: [URL]) throws -> Self {
    try Task.checkCancellation()
    guard Set(files.map(\.lastPathComponent)).count == files.count else {
      throw AirDropNetworkError.message("Choose files with different names for this transfer.")
    }
    let entries: [[String: Any]] = try files.map { url in
      let values = try url.resourceValues(forKeys: [.isDirectoryKey, .contentTypeKey])
      return ["FileName": url.lastPathComponent, "FileBomPath": "./" + url.lastPathComponent,
        "FileIsDirectory": values.isDirectory == true, "FileType": values.contentType?.identifier ?? UTType.data.identifier, "ConvertMediaFormats": 0]
    }
    let transferID = UUID()
    let ask = try PropertyListSerialization.data(fromPropertyList: [
      "SenderComputerName": Host.current().localizedName ?? "Crawler", "SenderModelName": "Mac",
      "SenderID": String(UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(12)),
      "BundleID": "studio.crawler.native-prototype", "ConvertMediaFormats": false, "Files": entries,
      "TransferID": ["id": transferID.uuidString], "TransferType": ["files": [String: String]()]
    ], format: .binary, options: 0)
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent("Crawler-AirDrop-" + UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let archive = directory.appendingPathComponent("files.cpio.gz")
    do {
      let process = Process()
      process.executableURL = URL(fileURLWithPath: "/usr/bin/tar")
      process.arguments = ["--format=cpio", "-czf", archive.path] + files.flatMap { ["-C", $0.deletingLastPathComponent().path, "./" + $0.lastPathComponent] }
      process.standardOutput = FileHandle.nullDevice
      let errorURL = directory.appendingPathComponent("archive-error")
      FileManager.default.createFile(atPath: errorURL.path, contents: nil)
      let errorFile = try FileHandle(forWritingTo: errorURL)
      defer { try? errorFile.close() }
      process.standardError = errorFile
      try process.run()
      while process.isRunning {
        if Task.isCancelled { process.terminate(); process.waitUntilExit(); throw CancellationError() }
        Thread.sleep(forTimeInterval: 0.05)
      }
      guard process.terminationStatus == 0 else { throw AirDropNetworkError.message("Couldn't prepare the selected files for AirDrop.") }
      return Self(directory: directory, archive: archive, ask: ask, transferID: transferID)
    } catch { try? FileManager.default.removeItem(at: directory); throw error }
  }
}

/// A self-issued sender certificate for Everyone mode, held only in memory.
/// This does not read or impersonate the user's Apple-account credentials.
struct AirDropSenderCertificate: Sendable {
  let data: Data
  let passphrase: String
}
actor AirDropSender {
  static let shared = AirDropSender()
  private var certificate: AirDropSenderCertificate?
  func connection(to endpoint: AirDropEndpoint) throws -> AirDropConnection {
    if let certificate { return try AirDropConnection(endpoint, certificate: certificate) }
    guard #available(macOS 15, *) else {
      throw AirDropNetworkError.message("Custom AirDrop sending requires macOS 15 or later.")
    }
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent("Crawler-AirDrop-Identity-" + UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
    defer { try? FileManager.default.removeItem(at: directory) }
    let key = directory.appendingPathComponent("sender-key.pem")
    let certificateURL = directory.appendingPathComponent("sender.pem")
    let bundle = directory.appendingPathComponent("sender.p12")
    let passphrase = UUID().uuidString
    for arguments in [
      ["req", "-newkey", "rsa:2048", "-nodes", "-keyout", key.path, "-x509", "-days", "365", "-out", certificateURL.path, "-subj", "/CN=Crawler"],
      ["pkcs12", "-export", "-inkey", key.path, "-in", certificateURL.path, "-out", bundle.path, "-keypbe", "PBE-SHA1-3DES", "-certpbe", "PBE-SHA1-3DES", "-macalg", "sha1", "-passout", "stdin"]
    ] {
      try Task.checkCancellation()
      let process = Process()
      process.executableURL = URL(fileURLWithPath: "/usr/bin/openssl")
      process.arguments = arguments
      process.standardOutput = FileHandle.nullDevice; process.standardError = FileHandle.nullDevice
      let input = Pipe()
      process.standardInput = input
      try process.run()
      if arguments.first == "pkcs12" { try input.fileHandleForWriting.write(contentsOf: Data((passphrase + "\n").utf8)) }
      try input.fileHandleForWriting.close()
      process.waitUntilExit()
      guard process.terminationStatus == 0 else { throw AirDropNetworkError.message("Couldn't create Crawler's AirDrop sender certificate.") }
    }
    let certificate = AirDropSenderCertificate(data: try Data(contentsOf: bundle), passphrase: passphrase)
    self.certificate = certificate
    return try AirDropConnection(endpoint, certificate: certificate)
  }
}
