import sys
from PIL import Image
d=sys.argv[1]; prev=None
for F in range(57,72):
    px=Image.open(f"{d}/{F:04d}.png").convert('RGB').load()
    pts=[(x,y) for y in range(300,720) for x in range(600,1280) if (lambda c: c[0]>225 and c[2]>215 and 140<c[1]<200)(px[x,y]) and not (410<=y<=430 and x<=760) and not (y<410 and x<640)]
    if not pts: print(F,'none'); continue
    xs=[p[0] for p in pts]; ys=[p[1] for p in pts]
    cx=sum(xs)/len(xs); cy=sum(ys)/len(ys)
    v='' if prev is None else f" v={((cx-prev[0])**2+(cy-prev[1])**2)**.5:.1f}px/f"
    print(f"F{F} cursor centroid=({cx:.0f},{cy:.0f}) bbox=({min(xs)},{min(ys)},{max(xs)},{max(ys)}) n={len(pts)}{v}")
    prev=(cx,cy)
