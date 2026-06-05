import 'package:flutter/material.dart';

import '../app_state.dart';
import '../generated/protocol.dart';

/// Zone editor — creates a new zone or edits an existing one (pass `existing`). A zone is
/// a name + one or more links; this screen assembles that link list, then POSTs (create)
/// or PUTs (edit). Volumes aren't touched here — an edit preserves them server-side.
///
/// "Add link" is a two-step pick: first a source (any output port), then a destination.
/// Destinations are filtered to input ports containing "playback" — the sink inputs you
/// actually route audio into (e.g. `playback_FL`/`FR`), keeping the picker to real targets.
class ZoneEditorScreen extends StatefulWidget {
  final AppState state;

  /// The zone to edit, or `null` to create a new one.
  final ZoneView? existing;

  const ZoneEditorScreen({super.key, required this.state, this.existing});

  bool get isEdit => existing != null;

  @override
  State<ZoneEditorScreen> createState() => _ZoneEditorScreenState();
}

class _ZoneEditorScreenState extends State<ZoneEditorScreen> {
  late final TextEditingController _name;
  late final List<LinkView> _links;
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    _name = TextEditingController(text: widget.existing?.name ?? '');
    // Copy into a local working list so add/remove only commit on Save.
    _links = [...?widget.existing?.links];
    // Re-run the Save-button enable/disable check as the user types.
    _name.addListener(() => setState(() {}));
  }

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  bool get _canSave =>
      !_saving && _name.text.trim().isNotEmpty && _links.isNotEmpty;

  @override
  Widget build(BuildContext context) {
    // Listen so the port lists stay fresh if the graph snapshot updates while open.
    return ListenableBuilder(
      listenable: widget.state,
      builder: (context, _) {
        final g = widget.state.graph;
        return Scaffold(
          appBar: AppBar(
            title: Text(widget.isEdit ? 'Edit zone' : 'New zone'),
            actions: [
              TextButton(
                onPressed: _canSave ? _save : null,
                child: const Text('Save'),
              ),
            ],
          ),
          body: g == null
              ? const Center(child: CircularProgressIndicator())
              : _form(context, g),
        );
      },
    );
  }

  Widget _form(BuildContext context, GraphState g) {
    return ListView(
      padding: const EdgeInsets.all(16),
      children: [
        TextField(
          controller: _name,
          autofocus: !widget.isEdit,
          textCapitalization: TextCapitalization.words,
          decoration: const InputDecoration(
            labelText: 'Zone name',
            hintText: 'e.g. Patio',
            border: OutlineInputBorder(),
          ),
        ),
        const SizedBox(height: 24),
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          children: [
            Text('Links', style: Theme.of(context).textTheme.titleMedium),
            TextButton.icon(
              onPressed: () => _addLink(g),
              icon: const Icon(Icons.add),
              label: const Text('Add link'),
            ),
          ],
        ),
        if (_links.isEmpty)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 12),
            child: Text('No links yet. Add at least one to route a source into a '
                'playback input.'),
          ),
        for (final l in _links)
          ListTile(
            dense: true,
            leading: const Icon(Icons.link),
            title: Text('${_short(l.outputPort)}  →  ${_short(l.inputPort)}'),
            trailing: IconButton(
              icon: const Icon(Icons.close),
              tooltip: 'Remove link',
              onPressed: () => setState(() => _links.remove(l)),
            ),
          ),
      ],
    );
  }

  // ---- link building -------------------------------------------------------

  Future<void> _addLink(GraphState g) async {
    // Uses the State's own `context`, so the `mounted` checks below are the correct guard.
    final out = await _pickPort(
      title: 'Source (output port)',
      empty: 'No output ports available.',
      options: _outputPorts(g),
    );
    if (out == null || !mounted) return;
    final inp = await _pickPort(
      title: 'Destination (playback input)',
      empty: 'No playback inputs available.',
      options: _playbackInputPorts(g),
    );
    if (inp == null) return;
    // Skip an exact duplicate of an already-added link.
    final dup =
        _links.any((l) => l.outputPort == out && l.inputPort == inp);
    if (!dup) {
      setState(() => _links.add(LinkView(outputPort: out, inputPort: inp)));
    }
  }

  List<_PortOption> _outputPorts(GraphState g) => [
        for (final n in g.nodes)
          for (final p in n.ports)
            if (p.direction == Direction.OUT) _PortOption(n.name, p),
      ];

  // The "playback on the destination side" filter: input ports whose name or stable
  // key contains "playback" (case-insensitive) — the sink inputs audio routes into.
  List<_PortOption> _playbackInputPorts(GraphState g) => [
        for (final n in g.nodes)
          for (final p in n.ports)
            if (p.direction == Direction.IN && _isPlayback(p))
              _PortOption(n.name, p),
      ];

  bool _isPlayback(PortView p) =>
      p.name.toLowerCase().contains('playback') ||
      p.key.toLowerCase().contains('playback');

  Future<String?> _pickPort({
    required String title,
    required String empty,
    required List<_PortOption> options,
  }) {
    return showModalBottomSheet<String>(
      context: context,
      showDragHandle: true,
      builder: (ctx) => SafeArea(
        child: ListView(
          shrinkWrap: true,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
              child: Text(title, style: Theme.of(ctx).textTheme.titleMedium),
            ),
            if (options.isEmpty)
              Padding(
                padding: const EdgeInsets.all(16),
                child: Text(empty),
              ),
            for (final o in options)
              ListTile(
                title: Text(o.port.name),
                subtitle: Text(o.nodeName),
                onTap: () => Navigator.pop(ctx, o.port.key),
              ),
          ],
        ),
      ),
    );
  }

  // ---- save ----------------------------------------------------------------

  Future<void> _save() async {
    setState(() => _saving = true);
    final name = _name.text.trim();
    try {
      if (widget.isEdit) {
        await widget.state.updateZone(widget.existing!.name, name, _links);
      } else {
        await widget.state.createZone(name, _links);
      }
      if (mounted) Navigator.pop(context);
    } catch (e) {
      if (!mounted) return;
      setState(() => _saving = false);
      final verb = widget.isEdit ? 'update' : 'create';
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Could not $verb zone: $e')),
      );
    }
  }

  // Stable keys are long; show the last segment for readability (matches Graph lens).
  String _short(String key) {
    final hash = key.lastIndexOf('#');
    return hash >= 0 ? key.substring(hash + 1) : key;
  }
}

/// A pickable port plus the name of the node it belongs to (for the picker subtitle).
class _PortOption {
  final String nodeName;
  final PortView port;
  _PortOption(this.nodeName, this.port);
}
