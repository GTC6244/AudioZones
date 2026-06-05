// To parse this JSON data, do
//
//     final graphState = graphStateFromJson(jsonString);

import 'dart:convert';

GraphState graphStateFromJson(String str) => GraphState.fromJson(json.decode(str));

String graphStateToJson(GraphState data) => json.encode(data.toJson());


///Everything a client needs to render, in one message.
class GraphState {
    
    ///True when the server is talking to a live PipeWire (false = degraded/mock).
    bool connected;
    List<LinkView> links;
    List<NodeView> nodes;
    List<ZoneView> zones;

    GraphState({
        required this.connected,
        required this.links,
        required this.nodes,
        required this.zones,
    });

    factory GraphState.fromJson(Map<String, dynamic> json) => GraphState(
        connected: json["connected"],
        links: List<LinkView>.from(json["links"].map((x) => LinkView.fromJson(x))),
        nodes: List<NodeView>.from(json["nodes"].map((x) => NodeView.fromJson(x))),
        zones: List<ZoneView>.from(json["zones"].map((x) => ZoneView.fromJson(x))),
    );

    Map<String, dynamic> toJson() => {
        "connected": connected,
        "links": List<dynamic>.from(links.map((x) => x.toJson())),
        "nodes": List<dynamic>.from(nodes.map((x) => x.toJson())),
        "zones": List<dynamic>.from(zones.map((x) => x.toJson())),
    };
}

class LinkView {
    String inputPort;
    String outputPort;

    LinkView({
        required this.inputPort,
        required this.outputPort,
    });

    factory LinkView.fromJson(Map<String, dynamic> json) => LinkView(
        inputPort: json["input_port"],
        outputPort: json["output_port"],
    );

    Map<String, dynamic> toJson() => {
        "input_port": inputPort,
        "output_port": outputPort,
    };
}

class NodeView {
    
    ///Raw-linear per-channel volumes in the node's channel order (empty if no volume control).
    ///Lets a client show/drive individual channels (e.g. "card ch 7-8 -> patio").
    List<double>? channelVolumes;
    
    ///Stable key: `media.class|node.name`. Survives reconnect; numeric PipeWire ids do not.
    ///Deliberately excludes `object.serial` (per-session, not durable). See `identity`.
    String key;
    String mediaClass;
    bool muted;
    String name;
    List<PortView> ports;
    
    ///False when the device is not currently present (unplugged); zones depending on it show
    ///"degraded".
    bool present;
    
    ///Representative level, 0.0..=1.0 (the max across channels). `None` if this node has no
    ///volume control. For a one-knob UI; per-channel detail lives in `channel_volumes`.
    double? volume;

    NodeView({
        this.channelVolumes,
        required this.key,
        required this.mediaClass,
        required this.muted,
        required this.name,
        required this.ports,
        required this.present,
        this.volume,
    });

    factory NodeView.fromJson(Map<String, dynamic> json) => NodeView(
        channelVolumes: json["channel_volumes"] == null ? [] : List<double>.from(json["channel_volumes"]!.map((x) => x?.toDouble())),
        key: json["key"],
        mediaClass: json["media_class"],
        muted: json["muted"],
        name: json["name"],
        ports: List<PortView>.from(json["ports"].map((x) => PortView.fromJson(x))),
        present: json["present"],
        volume: json["volume"]?.toDouble(),
    );

    Map<String, dynamic> toJson() => {
        "channel_volumes": channelVolumes == null ? [] : List<dynamic>.from(channelVolumes!.map((x) => x)),
        "key": key,
        "media_class": mediaClass,
        "muted": muted,
        "name": name,
        "ports": List<dynamic>.from(ports.map((x) => x.toJson())),
        "present": present,
        "volume": volume,
    };
}

class PortView {
    Direction direction;
    
    ///Stable key: `(node_key, port.name, direction)`.
    String key;
    String name;

    PortView({
        required this.direction,
        required this.key,
        required this.name,
    });

    factory PortView.fromJson(Map<String, dynamic> json) => PortView(
        direction: directionValues.map[json["direction"]]!,
        key: json["key"],
        name: json["name"],
    );

    Map<String, dynamic> toJson() => {
        "direction": directionValues.reverse[direction],
        "key": key,
        "name": name,
    };
}

enum Direction {
    IN,
    OUT
}

final directionValues = EnumValues({
    "in": Direction.IN,
    "out": Direction.OUT
});

class ZoneView {
    bool active;
    
    ///True when the zone is active but some of its devices/ports are missing.
    bool degraded;
    
    ///The zone's defined links (its routing recipe) — independent of what's currently live in
    ///the graph. Lets a client edit the zone (add/remove links) without a separate fetch.
    ///Distinct from top-level `GraphState.links`, which are live links.
    List<LinkView> links;
    
    ///Stable keys of devices the zone wants but can't currently reach.
    List<String> missing;
    
    ///Live mute state of `volume_node` (false when there's no volume node).
    bool muted;
    String name;
    
    ///Live representative volume (0.0..=1.0) of `volume_node`, if that node is present. `None`
    ///-> the tile shows no slider (node absent or zone has no volume node).
    double? volume;
    
    ///The zone's representative node — the sink whose volume the zone tile controls (the first
    ///volume-spec node, else the sink behind the zone's first link). `None` when the zone has
    ///no controllable node. Clients PUT volume changes to this key.
    String? volumeNode;

    ZoneView({
        required this.active,
        required this.degraded,
        required this.links,
        required this.missing,
        required this.muted,
        required this.name,
        this.volume,
        this.volumeNode,
    });

    factory ZoneView.fromJson(Map<String, dynamic> json) => ZoneView(
        active: json["active"],
        degraded: json["degraded"],
        links: List<LinkView>.from(json["links"].map((x) => LinkView.fromJson(x))),
        missing: List<String>.from(json["missing"].map((x) => x)),
        muted: json["muted"],
        name: json["name"],
        volume: json["volume"]?.toDouble(),
        volumeNode: json["volume_node"],
    );

    Map<String, dynamic> toJson() => {
        "active": active,
        "degraded": degraded,
        "links": List<dynamic>.from(links.map((x) => x.toJson())),
        "missing": List<dynamic>.from(missing.map((x) => x)),
        "muted": muted,
        "name": name,
        "volume": volume,
        "volume_node": volumeNode,
    };
}

class EnumValues<T> {
    Map<String, T> map;
    late Map<T, String> reverseMap;

    EnumValues(this.map);

    Map<T, String> get reverse {
            reverseMap = map.map((k, v) => MapEntry(v, k));
            return reverseMap;
    }
}
