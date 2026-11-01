import sys
from PIL import Image
d=sys.argv[1]
for F in range(56,72):
    px=Image.open(f"{d}/{F:04d}.png").convert('RGB').load()
    pts=[(x,y) for y in range(380,720) for x in range(600,1280) if (lambda c: c[0]>225 and c[2]>215 and 140<c[1]<200)(px[x,y])]
    # button left edge & height (magenta, x<600)
    bx=[x for y in range(250,450,3) for x in range(380,640) if (lambda c:c[0]>170 and c[1]<110 and c[2]>140)(px[x,y])]
    by=[y for y in range(200,470) for x in range(500,600,4) if (lambda c:c[0]>170 and c[1]<110 and c[2]>140)(px[x,y])]
    s=f"F{F} btn_left={min(bx) if bx else None} btn_y=({min(by) if by else None},{max(by) if by else None})"
    if pts:
        tip=min(pts,key=lambda p:p[0]+p[1]); cx=sum(p[0] for p in pts)/len(pts); cy=sum(p[1] for p in pts)/len(pts)
        s+=f" cursor centroid=({cx:.0f},{cy:.0f}) tip~{tip} n={len(pts)}"
    print(s)
