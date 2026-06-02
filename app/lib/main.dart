import 'package:flutter/material.dart';

import 'api_client.dart';
import 'app_state.dart';
import 'config.dart';
import 'screens/create_zone_screen.dart';
import 'screens/graph_screen.dart';
import 'screens/zones_screen.dart';
import 'theme.dart';

void main() => runApp(const AudioZonesApp());

class AudioZonesApp extends StatefulWidget {
  const AudioZonesApp({super.key});

  @override
  State<AudioZonesApp> createState() => _AudioZonesAppState();
}

class _AudioZonesAppState extends State<AudioZonesApp> {
  late final AppState _state = AppState(ApiClient(AppConfig.dev));

  @override
  void dispose() {
    _state.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'AudioZones',
      debugShowCheckedModeBanner: false,
      theme: buildTheme(),
      home: HomeShell(state: _state),
    );
  }
}

class HomeShell extends StatefulWidget {
  final AppState state;
  const HomeShell({super.key, required this.state});

  @override
  State<HomeShell> createState() => _HomeShellState();
}

class _HomeShellState extends State<HomeShell> {
  int _tab = 0;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: widget.state,
      builder: (context, _) {
        final s = widget.state;
        final body = _tab == 0 ? ZonesScreen(state: s) : GraphScreen(state: s);
        return Scaffold(
          appBar: AppBar(
            title: const Text('AudioZones'),
            bottom: _connectionBanner(s),
          ),
          body: body,
          // The "+" lives on the Zones tab only — Graph has its own tap-to-link flow.
          floatingActionButton: _tab == 0
              ? FloatingActionButton(
                  tooltip: 'New zone',
                  onPressed: () => Navigator.of(context).push(
                    MaterialPageRoute(
                      builder: (_) => CreateZoneScreen(state: s),
                    ),
                  ),
                  child: const Icon(Icons.add),
                )
              : null,
          bottomNavigationBar: NavigationBar(
            selectedIndex: _tab,
            onDestinationSelected: (i) => setState(() => _tab = i),
            destinations: const [
              NavigationDestination(
                icon: Icon(Icons.dashboard_outlined),
                selectedIcon: Icon(Icons.dashboard),
                label: 'Zones',
              ),
              NavigationDestination(
                icon: Icon(Icons.hub_outlined),
                selectedIcon: Icon(Icons.hub),
                label: 'Graph',
              ),
            ],
          ),
        );
      },
    );
  }

  /// "Connecting…" / "Reconnecting…" strip shown whenever we're not connected.
  /// Pairs with the greyed-out, non-interactive surfaces in the screens.
  PreferredSizeWidget? _connectionBanner(AppState s) {
    if (s.status == ConnStatus.connected) return null;
    final msg = s.status == ConnStatus.connecting ? 'Connecting…' : 'Reconnecting…';
    return PreferredSize(
      preferredSize: const Size.fromHeight(26),
      child: Container(
        width: double.infinity,
        color: kDegraded,
        padding: const EdgeInsets.symmetric(vertical: 4),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            const SizedBox(
              width: 13,
              height: 13,
              child: CircularProgressIndicator(strokeWidth: 2, color: Colors.white),
            ),
            const SizedBox(width: 8),
            Text(msg, style: const TextStyle(color: Colors.white)),
          ],
        ),
      ),
    );
  }
}
