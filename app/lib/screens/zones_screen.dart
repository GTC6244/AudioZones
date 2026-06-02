import 'package:flutter/material.dart';

import '../app_state.dart';
import '../generated/protocol.dart';
import '../theme.dart';

/// Zone lens — the primary, daily surface. Adaptive: a list of big tiles on a phone,
/// a multi-column grid on a tablet/landscape (the wall-panel form factor). On/off is
/// the glance target; degraded zones flag the missing device. When disconnected the
/// whole surface greys out and stops accepting taps (the connection banner explains).
class ZonesScreen extends StatelessWidget {
  final AppState state;
  const ZonesScreen({super.key, required this.state});

  @override
  Widget build(BuildContext context) {
    final g = state.graph;
    if (g == null) {
      return const Center(child: CircularProgressIndicator());
    }
    if (g.zones.isEmpty) {
      return _emptyState(context);
    }
    final disabled = !state.connected;
    return Opacity(
      opacity: disabled ? 0.5 : 1.0,
      child: IgnorePointer(
        ignoring: disabled,
        child: LayoutBuilder(
          builder: (context, constraints) {
            final cols = (constraints.maxWidth / 320).floor().clamp(1, 4);
            return GridView.count(
              crossAxisCount: cols,
              padding: const EdgeInsets.all(12),
              childAspectRatio: 2.6,
              mainAxisSpacing: 12,
              crossAxisSpacing: 12,
              children: [for (final z in g.zones) _ZoneTile(zone: z, state: state)],
            );
          },
        ),
      ),
    );
  }

  Widget _emptyState(BuildContext context) => Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.speaker_group_outlined, size: 48),
            const SizedBox(height: 12),
            const Text('No zones yet'),
            const SizedBox(height: 4),
            Text(
              'Define zones in zones.toml on the server.',
              style: Theme.of(context).textTheme.bodySmall,
            ),
          ],
        ),
      );
}

class _ZoneTile extends StatelessWidget {
  final ZoneView zone;
  final AppState state;
  const _ZoneTile({required this.zone, required this.state});

  @override
  Widget build(BuildContext context) {
    final statusLine = !zone.active
        ? 'Off'
        : (zone.degraded ? 'On · degraded' : 'On');
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    zone.name,
                    style: Theme.of(context).textTheme.titleMedium,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                if (zone.degraded)
                  Padding(
                    padding: const EdgeInsets.only(right: 8),
                    child: Tooltip(
                      message: 'Missing: ${zone.missing.join(", ")}',
                      child: const Icon(Icons.warning_amber_rounded,
                          color: kDegraded, semanticLabel: 'degraded'),
                    ),
                  ),
                Switch(
                  value: zone.active,
                  activeThumbColor: kAccentOn,
                  onChanged: (_) => state.toggleZone(zone),
                ),
              ],
            ),
            const Spacer(),
            // NOTE: zone-level volume needs a `volume` field on ZoneView (server-side
            // follow-up). Until then the tile owns on/off; per-node volume lives in the
            // Graph lens. The toggle is the core daily action, so this is still useful.
            Text(statusLine, style: Theme.of(context).textTheme.bodySmall),
          ],
        ),
      ),
    );
  }
}
