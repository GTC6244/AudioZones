import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

import 'api_client.dart';
import 'generated/protocol.dart';

enum ConnStatus { connecting, connected, reconnecting }

/// Holds the latest [GraphState] and connection status, and owns the WebSocket
/// lifecycle (connect → listen → auto-reconnect with backoff). The UI is a pure
/// function of this; commands go through [ApiClient] and the result comes back as
/// the next snapshot (server is source of truth — no optimistic local mutation yet).
class AppState extends ChangeNotifier {
  final ApiClient api;

  GraphState? graph;
  ConnStatus status = ConnStatus.connecting;

  WebSocketChannel? _channel;
  StreamSubscription<dynamic>? _sub;
  bool _disposed = false;

  AppState(this.api) {
    _connect();
  }

  bool get connected => status == ConnStatus.connected;

  void _connect() {
    if (_disposed) return;
    status = graph == null ? ConnStatus.connecting : ConnStatus.reconnecting;
    notifyListeners();
    try {
      _channel = api.connectWs();
      _sub = _channel!.stream.listen(
        (data) {
          try {
            graph = GraphState.fromJson(jsonDecode(data as String) as Map<String, dynamic>);
            status = ConnStatus.connected;
            notifyListeners();
          } catch (_) {
            // Ignore a malformed frame; the next snapshot will be whole.
          }
        },
        onError: (_) => _retry(),
        onDone: _retry,
        cancelOnError: true,
      );
    } catch (_) {
      _retry();
    }
  }

  void _retry() {
    if (_disposed) return;
    status = ConnStatus.reconnecting;
    notifyListeners();
    _sub?.cancel();
    _sub = null;
    Future.delayed(const Duration(seconds: 2), _connect);
  }

  // ---- commands (fire, then wait for the snapshot to reflect them) ----

  Future<void> toggleZone(ZoneView z) =>
      z.active ? api.deactivate(z.name) : api.activate(z.name);

  Future<void> setNodeVolume(NodeView n, double volume) =>
      api.setVolume(n.key, volume, n.muted);

  Future<void> toggleMute(NodeView n) =>
      api.setVolume(n.key, n.volume ?? 1.0, !n.muted);

  Future<void> createLink(String outputPort, String inputPort) =>
      api.createLink(outputPort, inputPort);

  Future<void> deleteLink(String outputPort, String inputPort) =>
      api.deleteLink(outputPort, inputPort);

  @override
  void dispose() {
    _disposed = true;
    _sub?.cancel();
    _channel?.sink.close();
    super.dispose();
  }
}
