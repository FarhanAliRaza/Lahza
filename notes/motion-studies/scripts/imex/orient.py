import sys, math
from PIL import Image
d=sys.argv[1]; a,b=int(sys.argv[2]),int(sys.argv[3])
W,H=320,180
prev=None
for n in range(a,b+1):
    im=Image.open(f"{d}/f{n:03d}.png").convert('RGB').resize((W,H),Image.BILINEAR)
    p=im.load()
    m=[[0]*W for _ in range(H)]
    cnt=0
    for y in range(H):
        for x in range(W):
            r,g,bb=p[x,y]
            if g>170 and bb>180 and r<170: m[y][x]=1;cnt+=1
    # structure tensor on mask
    jxx=jyy=jxy=0
    for y in range(1,H-1):
        for x in range(1,W-1):
            gx=m[y][x+1]-m[y][x-1]; gy=m[y+1][x]-m[y-1][x]
            jxx+=gx*gx;jyy+=gy*gy;jxy+=gx*gy
    # dominant gradient angle; edge (axis) direction is perpendicular
    th=0.5*math.atan2(2*jxy,jxx-jyy)
    ax=math.degrees(th)+90
    coh=math.sqrt((jxx-jyy)**2+4*jxy*jxy)/(jxx+jyy+1e-9)
    # axis angle measured from +x, screen y down -> convert to math (counterclockwise positive)
    ang=-((ax+90)%180-90)
    print(n, f"cyan={100*cnt/(W*H):5.1f}%", f"axis={ang:7.1f}deg", f"coh={coh:.2f}")
