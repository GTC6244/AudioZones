import 'package:flutter/material.dart';

/// Minimal neutral design language (eng/design-review decision). The full color +
/// type system comes later via /design-consultation. One accent is reserved for the
/// "on / active" state so "is this zone on?" is answerable at a glance.
const Color kAccentOn = Color(0xFF2E9E6B);
const Color kDegraded = Color(0xFFB8860B);

ThemeData buildTheme() {
  final scheme = ColorScheme.fromSeed(seedColor: const Color(0xFF37474F));
  return ThemeData(
    useMaterial3: true,
    colorScheme: scheme,
    // Comfortable density keeps touch targets >= 44px (a11y decision).
    visualDensity: VisualDensity.comfortable,
  );
}
