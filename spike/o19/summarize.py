"""Summarize deck_spike --headless output (stdin) into one line. Usage: summarize.py <label>"""
import re
import statistics
import sys

label = sys.argv[1]
rows = [l for l in sys.stdin.read().splitlines() if l.startswith(('UP', 'DOWN'))]
avg, p95, mx = [], [], []
for l in rows:
    m = re.search(r'RTT ([\d.]+)/([\d.]+)/([\d.]+)/([\d.]+) ms', l)
    if m and l.startswith('UP'):
        avg.append(float(m[2]))
        p95.append(float(m[3]))
        mx.append(float(m[4]))
last = rows[-1] if rows else ''
lost = re.search(r'lost (\d+)', last)
stale = re.search(r'stale (\d+)', last)
down = sum(1 for l in rows if l.startswith('DOWN'))
nan = float('nan')
print(f'{label:<22} rtt_avg {statistics.median(avg) if avg else nan:7.1f}  '
      f'rtt_p95 {max(p95) if p95 else nan:7.1f}  rtt_max {max(mx) if mx else nan:7.1f} ms  '
      f'lost {lost[1] if lost else "-":>4}  stale {stale[1] if stale else "-":>4}  '
      f'down_s {down:>2}  ({len(rows)} s)')
