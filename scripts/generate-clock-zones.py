from zoneinfo import ZoneInfo
from datetime import datetime, timezone
from pathlib import Path
zones=['UTC','America/Los_Angeles','America/Denver','America/Chicago','America/New_York','America/Sao_Paulo','Europe/London','Europe/Paris','Asia/Shanghai','Asia/Tokyo','Asia/Kolkata','Australia/Sydney']
start=0; end=4133980800
out=['// Generated from IANA zoneinfo using scripts/generate-clock-zones.py.','// Inclusive epoch range: 1970-01-01 through 2100-12-31 UTC.','pub const ZONES: &[(&str, &[(i64, i32)])] = &[']
for name in zones:
 z=ZoneInfo(name)
 def offset(t): return int(datetime.fromtimestamp(t,timezone.utc).astimezone(z).utcoffset().total_seconds())
 prev=offset(start); transitions=[(start,prev)]; t=86400
 while t<end:
  cur=offset(t)
  if cur!=prev:
   lo=t-86400; hi=t
   while hi-lo>1:
    mid=(lo+hi)//2
    if offset(mid)==prev: lo=mid
    else: hi=mid
   transitions.append((hi,cur)); prev=cur
  t+=86400
 out.append('    ("'+name+'", &[')
 out.extend(f'        ({t}, {o}),' for t,o in transitions)
 out.append('    ]),')
out.append('];')
Path('crates/alloyos-js/data/clock_zones.rs').write_text('\n'.join(out)+'\n')
