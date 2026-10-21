// Human-friendly sizes, speeds and times.

String formatSize(int bytes) {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  var value = bytes.toDouble();
  var unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  if (unit == 0) return '$bytes B';
  return '${value.toStringAsFixed(value >= 100 ? 0 : 1)} ${units[unit]}';
}

String formatSpeed(double bytesPerSec) => '${formatSize(bytesPerSec.round())}/s';

/// "8 s", "2 min", "1 h 5 min".
String formatDuration(Duration d) {
  if (d.inSeconds < 60) return '${d.inSeconds.clamp(1, 59)} s';
  if (d.inMinutes < 60) return '${d.inMinutes} min';
  final minutes = d.inMinutes % 60;
  return minutes == 0 ? '${d.inHours} h' : '${d.inHours} h $minutes min';
}
