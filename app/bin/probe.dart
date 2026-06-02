// Headless connectivity probe: uses the app's real ApiClient + generated models to
// connect to the configured server over WebSocket and print the first snapshot, then
// flips a zone and a node volume. Proves the app's networking talks to the box.
//
//   cd app && dart run bin/probe.dart
//
// Not part of the app build — a dev/CI smoke check.
// ignore_for_file: avoid_print

import 'dart:async';
import 'dart:convert';

import 'package:audiozones/api_client.dart';
import 'package:audiozones/config.dart';
import 'package:audiozones/generated/protocol.dart';

Future<void> main(List<String> argv) async {
  // Optional override: dart run bin/probe.dart <wsBase> <httpBase> [token]
  final cfg = argv.length >= 2
      ? AppConfig(
          wsBase: argv[0],
          httpBase: argv[1],
          token: argv.length >= 3 ? argv[2] : 'dev-token',
        )
      : AppConfig.dev;
  print('connecting to ${cfg.wsBase}/ws ...');
  final api = ApiClient(cfg);
  final ch = api.connectWs();

  final first = Completer<String>();
  final sub = ch.stream.listen(
    (d) {
      if (!first.isCompleted) first.complete(d as String);
    },
    onError: (e) {
      if (!first.isCompleted) first.completeError(e);
    },
  );

  final data = await first.future.timeout(const Duration(seconds: 5));
  final g = GraphState.fromJson(jsonDecode(data) as Map<String, dynamic>);

  print('CONNECTED — connected=${g.connected} nodes=${g.nodes.length} '
      'links=${g.links.length} zones=${g.zones.length}');
  for (final n in g.nodes.where((n) => n.mediaClass == 'Audio/Sink')) {
    print('  sink: ${n.name}  ports=${n.ports.length} vol=${n.volume}');
  }

  await sub.cancel();
  await ch.sink.close();
  print('OK — the app can read the live graph from the box.');
}
