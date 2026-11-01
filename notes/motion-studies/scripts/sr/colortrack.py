# colortrack.py dir F0 F1 rmin rmax gmin gmax bmin bmax [x0 y0 x1 y1]
import sys
from PIL import Image
d=sys.argv[1]; a,b=int(sys.argv[2]),int(sys.argv[3]); r0,r1,g0,g1,b0,b1=map(int,sys.argv[4:10])
box=list(map(int,sys.argv[10:14])) if len(sys.argv)>13 else [0,0,1280,720]
prev=None
for F in range(a,b+1):
    px=Image.open(f"{d}/{F:04d}.png").convert('RGB').load()
    pts=[(x,y) for y in range(box[1],box[3],2) for x in range(box[0],box[2],2) if r0<=px[x,y][0]<=r1 and g0<=px[x,y][1]<=g1 and b0<=px[x,y][2]<=b1]
    if len(pts)<20: print(f"F{F} none"); prev=None; continue
    xs=[p[0] for p in pts]; ys=[p[1] for p in pts]; cx=sum(xs)/len(xs); cy=sum(ys)/len(ys)
    v='' if prev is None else f" vx={cx-prev[0]:+.1f} vy={cy-prev[1]:+.1f}"
    print(f"F{F} c=({cx:.0f},{cy:.0f}) bbox=({min(xs)},{min(ys)},{max(xs)},{max(ys)}) n={len(pts)}{v}")
    prev=(cx,cy)
