/// Where the AudioZones server lives and the bearer token to reach it.
///
/// Dev defaults target a server running on this machine. Notes for real devices:
///  - Android emulator reaches the host at 10.0.2.2, not 127.0.0.1.
///  - A physical phone needs the server's LAN IP (e.g. 192.168.1.x).
/// A settings screen to set these at runtime is a follow-up.
class AppConfig {
  final String httpBase;
  final String wsBase;
  final String token;

  const AppConfig({
    required this.httpBase,
    required this.wsBase,
    required this.token,
  });

  // Points at the AudioZones server on the home-media box (LAN). For an Android
  // emulator use 10.0.2.2; for a different box change the IP. A runtime settings
  // screen is a follow-up.
  static const dev = AppConfig(
    httpBase: 'http://192.168.1.25:4040',
    wsBase: 'ws://192.168.1.25:4040',
    token: 'dev-token',
  );
}
