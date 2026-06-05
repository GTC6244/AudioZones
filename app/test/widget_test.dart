// Protocol round-trip test for the generated models. Deterministic, no network.

import 'package:audiozones/generated/protocol.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('GraphState parses a server snapshot', () {
    const json = '''
    {
      "connected": true,
      "nodes": [
        {"key":"Audio/Sink|Amp|S1","name":"Amp","media_class":"Audio/Sink",
         "ports":[{"key":"Audio/Sink|Amp|S1#in:playback_FL","name":"playback_FL","direction":"in"}],
         "volume":0.5,"channel_volumes":[0.5,0.5],"muted":false,"present":true}
      ],
      "links": [
        {"output_port":"Audio/Source|Cap|C1#out:capture_FL","input_port":"Audio/Sink|Amp|S1#in:playback_FL"}
      ],
      "zones": [
        {"name":"patio","active":true,"degraded":false,"missing":[],
         "links":[{"output_port":"Audio/Source|Cap|C1#out:capture_FL","input_port":"Audio/Sink|Amp|S1#in:playback_FL"}],
         "volume_node":"Audio/Sink|Amp|S1","volume":0.5,"muted":false}
      ]
    }''';

    final g = graphStateFromJson(json);

    expect(g.connected, isTrue);
    expect(g.nodes.single.volume, 0.5);
    expect(g.nodes.single.channelVolumes, [0.5, 0.5]);
    expect(g.nodes.single.ports.single.direction, Direction.IN);
    expect(g.links.single.inputPort, contains('playback_FL'));
    expect(g.zones.single.name, 'patio');
    expect(g.zones.single.active, isTrue);
    expect(g.zones.single.volumeNode, 'Audio/Sink|Amp|S1');
    expect(g.zones.single.volume, 0.5);
    expect(g.zones.single.links.single.inputPort, contains('playback_FL'));
  });

  test('GraphState survives a round-trip', () {
    final g = GraphState(
      connected: false,
      nodes: [],
      links: [
        LinkView(outputPort: 'a#out:x', inputPort: 'b#in:y'),
      ],
      zones: [
        ZoneView(
          name: 'kitchen',
          active: false,
          degraded: true,
          missing: ['b'],
          muted: false,
          links: [LinkView(outputPort: 'a#out:x', inputPort: 'b#in:y')],
        ),
      ],
    );
    final back = graphStateFromJson(graphStateToJson(g));
    expect(back.links.single.outputPort, 'a#out:x');
    expect(back.zones.single.degraded, isTrue);
    expect(back.zones.single.missing, ['b']);
    expect(back.zones.single.links.single.outputPort, 'a#out:x');
  });
}
