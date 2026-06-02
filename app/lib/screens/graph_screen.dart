import 'package:flutter/material.dart';

import '../app_state.dart';
import '../generated/protocol.dart';
import '../theme.dart';

/// Graph lens — power/setup. Read-mostly with a simple "tap two ports to link" model
/// (works on a phone; comfortable on a tablet in landscape). Each node shows its
/// volume/mute and its ports as chips. Tap an OUT port to arm it, then tap an IN port
/// to create the link. The links list lets you remove links. The full drag editor is
/// a later milestone.
class GraphScreen extends StatefulWidget {
  final AppState state;
  const GraphScreen({super.key, required this.state});

  @override
  State<GraphScreen> createState() => _GraphScreenState();
}

class _GraphScreenState extends State<GraphScreen> {
  String? _armedOut; // output-port key waiting to be linked to an input port

  @override
  Widget build(BuildContext context) {
    final g = widget.state.graph;
    if (g == null) {
      return const Center(child: CircularProgressIndicator());
    }
    final disabled = !widget.state.connected;
    return Opacity(
      opacity: disabled ? 0.5 : 1.0,
      child: IgnorePointer(
        ignoring: disabled,
        child: ListView(
          padding: const EdgeInsets.all(12),
          children: [
            if (_armedOut != null)
              Card(
                color: Theme.of(context).colorScheme.secondaryContainer,
                child: ListTile(
                  leading: const Icon(Icons.cable),
                  title: Text('Linking from ${_short(_armedOut!)}'),
                  subtitle: const Text('Tap an input port to connect, or cancel.'),
                  trailing: TextButton(
                    onPressed: () => setState(() => _armedOut = null),
                    child: const Text('Cancel'),
                  ),
                ),
              ),
            for (final n in g.nodes) _nodeCard(context, n),
            const SizedBox(height: 8),
            Text('Links', style: Theme.of(context).textTheme.titleMedium),
            if (g.links.isEmpty)
              const Padding(
                padding: EdgeInsets.all(8),
                child: Text('No links yet.'),
              ),
            for (final l in g.links)
              ListTile(
                dense: true,
                leading: const Icon(Icons.link),
                title: Text('${_short(l.outputPort)}  →  ${_short(l.inputPort)}'),
                trailing: IconButton(
                  icon: const Icon(Icons.link_off),
                  tooltip: 'Remove link',
                  onPressed: () => widget.state.deleteLink(l.outputPort, l.inputPort),
                ),
              ),
          ],
        ),
      ),
    );
  }

  Widget _nodeCard(BuildContext context, NodeView n) {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  n.mediaClass.contains('Source') ? Icons.input : Icons.speaker,
                  size: 18,
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    n.name,
                    style: Theme.of(context).textTheme.titleSmall,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                if (!n.present)
                  const Padding(
                    padding: EdgeInsets.only(left: 8),
                    child: Text('offline', style: TextStyle(color: kDegraded)),
                  ),
              ],
            ),
            if (n.volume != null) _volumeRow(n),
            const SizedBox(height: 8),
            Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [for (final p in n.ports) _portChip(p)],
            ),
          ],
        ),
      ),
    );
  }

  Widget _volumeRow(NodeView n) {
    return Row(
      children: [
        IconButton(
          icon: Icon(n.muted ? Icons.volume_off : Icons.volume_up),
          tooltip: n.muted ? 'Unmute' : 'Mute',
          onPressed: () => widget.state.toggleMute(n),
        ),
        Expanded(
          child: Slider(
            value: (n.volume ?? 0).clamp(0.0, 1.0),
            onChanged: (v) => widget.state.setNodeVolume(n, v),
          ),
        ),
        SizedBox(
          width: 40,
          child: Text('${((n.volume ?? 0) * 100).round()}%', textAlign: TextAlign.end),
        ),
      ],
    );
  }

  Widget _portChip(PortView p) {
    final isOut = p.direction == Direction.OUT;
    final armed = _armedOut == p.key;
    return ActionChip(
      avatar: Icon(isOut ? Icons.north_east : Icons.south_west, size: 16),
      label: Text(p.name),
      backgroundColor: armed ? kAccentOn.withValues(alpha: 0.25) : null,
      onPressed: () => _onPortTap(p),
    );
  }

  void _onPortTap(PortView p) {
    if (p.direction == Direction.OUT) {
      setState(() => _armedOut = (_armedOut == p.key) ? null : p.key);
    } else {
      // input port: complete a pending link if one is armed
      final out = _armedOut;
      if (out != null) {
        widget.state.createLink(out, p.key);
        setState(() => _armedOut = null);
      }
    }
  }

  // Stable keys are long; show the last segment for readability.
  String _short(String key) {
    final hash = key.lastIndexOf('#');
    return hash >= 0 ? key.substring(hash + 1) : key;
  }
}
