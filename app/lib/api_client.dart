import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:web_socket_channel/web_socket_channel.dart';

import 'config.dart';

/// Thin REST + WebSocket client for the AudioZones server. Commands go over REST;
/// state arrives as full snapshots over the WebSocket (snapshot-only protocol).
/// The bearer token gates both (header for REST, `?token=` for the WS).
class ApiClient {
  final AppConfig cfg;
  ApiClient(this.cfg);

  Map<String, String> get _headers => {
        'Authorization': 'Bearer ${cfg.token}',
        'Content-Type': 'application/json',
      };

  Future<void> activate(String zone) =>
      _send(http.post(_uri('/zones/${_enc(zone)}/activate'), headers: _headers));

  Future<void> deactivate(String zone) => _send(
      http.post(_uri('/zones/${_enc(zone)}/deactivate'), headers: _headers));

  Future<void> setVolume(String nodeKey, double volume, bool muted) => _send(http.put(
        _uri('/nodes/${_enc(nodeKey)}/volume'),
        headers: _headers,
        body: jsonEncode({'volume': volume, 'muted': muted}),
      ));

  Future<void> createLink(String outputPort, String inputPort) => _send(http.post(
        _uri('/links'),
        headers: _headers,
        body: jsonEncode({'output_port': outputPort, 'input_port': inputPort}),
      ));

  Future<void> deleteLink(String outputPort, String inputPort) => _send(http.delete(
        _uri('/links'),
        headers: _headers,
        body: jsonEncode({'output_port': outputPort, 'input_port': inputPort}),
      ));

  /// Await a command response and throw on any non-2xx status so callers don't
  /// silently treat a rejected command as success.
  Future<void> _send(Future<http.Response> req) async {
    final res = await req;
    if (res.statusCode < 200 || res.statusCode >= 300) {
      throw ApiException(res.statusCode, res.body);
    }
  }

  WebSocketChannel connectWs() =>
      WebSocketChannel.connect(Uri.parse('${cfg.wsBase}/ws?token=${_enc(cfg.token)}'));

  Uri _uri(String path) => Uri.parse('${cfg.httpBase}$path');

  // Stable keys contain '|', '#', ':' and spaces — must be percent-encoded in the path.
  String _enc(String s) => Uri.encodeComponent(s);
}

/// Raised when a command returns a non-2xx status, carrying the server's reason.
class ApiException implements Exception {
  final int statusCode;
  final String body;
  ApiException(this.statusCode, this.body);

  @override
  String toString() => 'ApiException($statusCode): $body';
}
