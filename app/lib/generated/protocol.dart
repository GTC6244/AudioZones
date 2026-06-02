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
    
    ///Stable key: `(node.name, media.class)[+serial]`. Survives reconnect; numeric PipeWire ids
    ///do not. See `identity`.
    String key;
    String mediaClass;
    bool muted;
    String name;
    List<PortView> ports;
    
    ///False when the device is not currently present (unplugged); zones depending on it show
    ///"degraded".
    bool present;
    
    ///0.0..=1.0. `None` if this node has no volume control.
    double? volume;

    NodeView({
        required this.key,
        required this.mediaClass,
        required this.muted,
        required this.name,
        required this.ports,
        required this.present,
        this.volume,
    });

    factory NodeView.fromJson(Map<String, dynamic> json) => NodeView(
        key: json["key"],
        mediaClass: json["media_class"],
        muted: json["muted"],
        name: json["name"],
        ports: List<PortView>.from(json["ports"].map((x) => PortView.fromJson(x))),
        present: json["present"],
        volume: json["volume"]?.toDouble(),
    );

    Map<String, dynamic> toJson() => {
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
    
    ///Stable keys of devices the zone wants but can't currently reach.
    List<String> missing;
    String name;

    ZoneView({
        required this.active,
        required this.degraded,
        required this.missing,
        required this.name,
    });

    factory ZoneView.fromJson(Map<String, dynamic> json) => ZoneView(
        active: json["active"],
        degraded: json["degraded"],
        missing: List<String>.from(json["missing"].map((x) => x)),
        name: json["name"],
    );

    Map<String, dynamic> toJson() => {
        "active": active,
        "degraded": degraded,
        "missing": List<dynamic>.from(missing.map((x) => x)),
        "name": name,
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
