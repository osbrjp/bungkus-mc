# bungkus mascot: poses and animated GIF, built from the Figma parts in
# file HwlCHEFqRm9hfOfUbtuL4h, frame 17:3 ("bungkys-mascott"):
#   idle 18:59 · look-up-left 18:108 · look-up-right 18:127 · duck 18:86 · died 18:60
# Usage: python3 gen.py  -> poses/*.svg + frames/f*.svg
# Then:  for f in frames/*.svg; do resvg -w 360 "$f" "${f%.svg}.png"; done
#        magick -delay 6 -loop 0 frames/*.png -layers Optimize raw.gif && gifsicle -O3 --colors 48 raw.gif -o mascot.gif
import os
BACK = "M289.206 60.4871C309.057 31.4093 351.943 31.4093 371.794 60.4871L563.34 341.058C585.998 374.246 562.23 419.25 522.046 419.25H138.954C98.7697 419.25 75.0025 374.246 97.6599 341.058L289.206 60.4871Z"
FACE = "M276.202 68.5094C296.053 39.4244 338.947 39.4245 358.798 68.5095L556.421 358.064C579.072 391.252 555.305 436.25 515.123 436.25H119.877C79.6956 436.25 55.9276 391.252 78.5791 358.064L276.202 68.5094Z"
WRAP_L = "M92.476 161.99L436.105 386.548L69.8177 571.86Z"     # #D9C574
WRAP_R = "M525.3 178.145L660.95 594.041L160.019 418.853Z"    # #E7D075
FEET = 485  # ground line in pose coordinates

EYES = {
    'open':  '<rect x="241.543" y="180" width="30" height="60" rx="5"/><rect x="335.457" y="180" width="30" height="60" rx="5"/>',
    'blink': '<rect x="241.543" y="203" width="30" height="14" rx="5"/><rect x="335.457" y="203" width="30" height="14" rx="5"/>',
    'left':  '<rect x="232" y="110" width="30" height="60" rx="5"/><rect x="325.913" y="110" width="30" height="60" rx="5"/>',
    'right': '<rect x="301" y="110" width="30" height="60" rx="5"/><rect x="394.913" y="110" width="30" height="60" rx="5"/>',
    'cross': ''.join(f'<rect x="{x+43.841}" y="162" width="16.4088" height="62" rx="5" transform="rotate(45 {x+43.841} 162)"/>'
                     f'<rect x="{x}" y="173.603" width="16.4088" height="62" rx="5" transform="rotate(-45 {x} 173.603)"/>' for x in (241, 335)),
}

def leg(x, facing, lift=0):
    b = FEET - lift
    if facing == 'left':   # foot points left, leg's bottom-right corner rounded
        return (f'<path d="M{x} 380H{x+18}V{b-5}c0 2.761-2.239 5-5 5H{x}Z"/>'
                f'<path d="M{x-8} {b}c-2.761 0-5-2.239-5-5v-3c0-2.761 2.239-5 5-5h18v13z"/>')
    return (f'<path d="M{x+18} 380H{x}V{b-5}c0 2.761 2.239 5 5 5H{x+18}Z"/>'
            f'<path d="M{x+26} {b}c2.761 0 5-2.239 5-5v-3c0-2.761-2.239-5-5-5h-18v13z"/>')

def pose(eyes='open', facing='left', crouch=0, squash=1.0, lift=(0, 0)):
    """crouch: body lowered onto the legs (duck = 43). Legs are drawn first, so the body hides their tops."""
    xs = (265, 390) if facing == 'left' else (252, 377)
    legs = leg(xs[0], facing, lift[0]) + leg(xs[1], facing, lift[1])
    body = (f'<g transform="translate(330.5 {419+crouch}) scale({squash} {1/squash}) translate(-330.5 -419)">'
            f'<mask id="m" maskUnits="userSpaceOnUse" x="40" y="0" width="600" height="480"><path d="{BACK}" fill="#fff"/></mask>'
            f'<g mask="url(#m)"><path d="{BACK}" fill="#2B9345"/><path d="{FACE}" fill="#34AB52"/>'
            f'<path d="{WRAP_L}" fill="#D9C574"/><path d="{WRAP_R}" fill="#E7D075"/></g>'
            f'<g fill="#000">{EYES[eyes]}</g></g>')
    return f'<g fill="#000">{legs}</g>{body}'

def svg(inner, air=0, bg=True, shadow=1.0):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="40 -20 580 540">'
            + ('<rect x="40" y="-20" width="580" height="540" fill="#F2F4D9"/>' if bg else '')
            + f'<ellipse cx="330" cy="{FEET+6}" rx="{150*shadow}" ry="8" fill="#DCE0C0"/>'
            + f'<g transform="translate(0 {air})">{inner}</g></svg>')

POSES = {
    'idle': dict(), 'blink': dict(eyes='blink'),
    'look-up-left': dict(eyes='left'), 'look-up-right': dict(eyes='right', facing='right'),
    'duck': dict(crouch=43, squash=1.03), 'died': dict(eyes='cross'),
}
os.makedirs('poses', exist_ok=True); os.makedirs('frames', exist_ok=True)
for name, kw in POSES.items():
    open(f'poses/{name}.svg', 'w').write(svg(pose(**kw)))

# 60 ms per frame, ~3.2 s loop
timeline = (['idle'] * 10 + ['look-up-left'] * 8 + ['idle'] * 3 + ['look-up-right'] * 8 + ['idle'] * 3
            + ['blink'] * 2 + ['idle'] * 4 + ['duck'] * 3)
frames = [(POSES[p], 0, 1.0) for p in timeline]
for air in (-14, -30, -42, -46, -42, -30, -14):          # hop
    frames.append((dict(squash=0.97), air, 1 + air / 120))
frames += [(POSES['duck'], 0, 1.0)] * 2 + [(POSES['idle'], 0, 1.0)] * 4
for i, (kw, air, sh) in enumerate(frames):
    open(f'frames/f{i:03d}.svg', 'w').write(svg(pose(**kw), air, shadow=sh))
print(len(frames), 'frames')
