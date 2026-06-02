import 'dart:math' as math;

/// Volume display mapping.
///
/// PipeWire `channelVolumes` are RAW-LINEAR amplitudes, but loudness perception is roughly
/// cubic — a raw 0.0156 sounds like "25%", not "1.5%". So sliders operate in PERCEPTUAL
/// space (0..1) and we convert at the edges: `perceptual = raw^(1/3)`, `raw = perceptual^3`.
/// This is the same cube-root curve PulseAudio/GNOME use, so our 25% matches `wpctl`'s 25%.
double rawToPerceptual(double raw) =>
    raw <= 0 ? 0.0 : math.pow(raw.clamp(0.0, 1.0), 1 / 3).toDouble();

double perceptualToRaw(double perceptual) =>
    math.pow(perceptual.clamp(0.0, 1.0), 3).toDouble();

/// A 0-100 label for a raw-linear volume, in the perceptual terms the user expects.
int perceptualPercent(double raw) => (rawToPerceptual(raw) * 100).round();
