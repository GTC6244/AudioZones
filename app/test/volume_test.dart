// Perceptual volume mapping (cube-root). Pure math, no network.

import 'package:audiozones/volume.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('raw<->perceptual round-trips', () {
    for (final p in [0.0, 0.1, 0.25, 0.5, 0.8, 1.0]) {
      expect(rawToPerceptual(perceptualToRaw(p)), closeTo(p, 1e-6));
    }
  });

  test('a raw 0.0156 reads as ~25% (matches wpctl/GNOME)', () {
    expect(perceptualPercent(0.0156), 25);
  });

  test('endpoints and out-of-range are clamped', () {
    expect(rawToPerceptual(0), 0.0);
    expect(rawToPerceptual(1), 1.0);
    expect(perceptualToRaw(0), 0.0);
    expect(perceptualToRaw(1), 1.0);
    expect(rawToPerceptual(-0.5), 0.0);
    expect(perceptualToRaw(2.0), 1.0);
  });
}
