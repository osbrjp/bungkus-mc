# Animated bungkus mascot, geometry exported from Figma node 17:3 ("bungkys-mascott").
# Render: python3 gen.py && for f in f*.svg; do resvg -w 360 "$f" "${f%.svg}.png"; done
#         magick -delay 6 -loop 0 f*.png -layers Optimize raw.gif && gifsicle -O3 --colors 48 raw.gif -o mascot.gif
import math
N = 36                      # frames at 60 ms -> 2.16 s loop
BACK = "M1008.21 485.487C1028.06 456.409 1070.94 456.409 1090.79 485.487L1282.34 766.058C1305 799.246 1281.23 844.25 1241.05 844.25H857.954C817.77 844.25 794.002 799.246 816.66 766.058L1008.21 485.487Z"
FACE = "M995.202 493.509C1015.05 464.424 1057.95 464.424 1077.8 493.509L1275.42 783.064C1298.07 816.252 1274.3 861.25 1234.12 861.25H838.877C798.696 861.25 774.928 816.252 797.579 783.064L995.202 493.509Z"
CX, BASE, GROUND = 1049.5, 844, 910

def leg(x, top, lift):
    foot = GROUND - lift        # bottom of foot
    return (f'<rect x="{x}" y="{top}" width="18" height="{foot-top}" fill="#000"/>'
            f'<path d="M{x-8} {foot}c-2.76 0-5-2.24-5-5v-3c0-2.76 2.24-5 5-5h18v13z" fill="#000"/>')

def frame(i):
    t = i / N * 2 * math.pi
    hop = max(0, math.sin(2 * t))
    bob = -16 * hop
    squash = 1 + 0.04 * math.cos(2 * t)
    blink = i in (26, 27, 28)
    ey, eh = (628, 14) if blink else (605, 60)
    lL, lR = 12 * max(0, math.sin(t)), 12 * max(0, -math.sin(t))
    body = f'translate({CX} {BASE+bob}) scale({squash} {1/squash}) translate({-CX} {-BASE})'
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="760 420 580 540">
<rect x="760" y="420" width="580" height="540" fill="#F2F4D9"/>
<ellipse cx="{CX}" cy="{GROUND+8}" rx="{190 - 40*hop}" ry="9" fill="#DCE0C0"/>
{leg(984, BASE+bob, lL)}{leg(1109, BASE+bob, lR)}
<g transform="{body}">
<mask id="m" maskUnits="userSpaceOnUse" x="760" y="420" width="580" height="540"><path d="{BACK}" fill="#fff"/></mask>
<g mask="url(#m)">
<path d="{BACK}" fill="#2B9345"/><path d="{FACE}" fill="#34AB52"/>
<rect x="960.543" y="{ey}" width="30" height="{eh}" rx="5" fill="#000"/><rect x="1054.46" y="{ey}" width="30" height="{eh}" rx="5" fill="#000"/>
<path d="M811.476 586.99L1155.1 811.548L788.818 996.86Z" fill="#E7D075"/>
<path d="M1244.3 603.145L1379.95 1019.04L879.019 843.853Z" fill="#D9C574"/>
</g></g></svg>'''

for i in range(N):
    open(f'f{i:02d}.svg', 'w').write(frame(i))
