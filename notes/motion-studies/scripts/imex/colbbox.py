import sys
from PIL import Image
# colbbox.py dir a b r,g,b tol [x0 y0 x1 y1]
d=sys.argv[1]; a,b=int(sys.argv[2]),int(sys.argv[3]); c=tuple(int(v) for v in sys.argv[4].split(',')); tol=int(sys.argv[5])
box=tuple(int(v) for v in sys.argv[6:10]) if len(sys.argv)>6 else None
for n in range(a,b+1):
    im=Image.open(f"{d}/f{n:03d}.png").convert('RGB')
    if box: im=im.crop(box)
    r,g,bb=im.split()
    m=Image.merge('RGB',(r,g,bb)).point(lambda v:v)
    px=im.load(); W,H=im.size
    xs=[];ys=[];cnt=0;sx=sy=0
    for y in range(0,H,2):
        for x in range(0,W,2):
            q=px[x,y]
            if abs(q[0]-c[0])+abs(q[1]-c[1])+abs(q[2]-c[2])<tol:
                cnt+=1;sx+=x;sy+=y;xs.append(x);ys.append(y)
    if cnt:
        ox,oy=(box[0],box[1]) if box else (0,0)
        area=cnt*4
        print(n,f"bbox x{min(xs)+ox}-{max(xs)+ox} y{min(ys)+oy}-{max(ys)+oy} w{max(xs)-min(xs)+2} h{max(ys)-min(ys)+2} area{area} req{(area/3.14159)**0.5:.1f} c({sx/cnt+ox:.0f},{sy/cnt+oy:.0f})")
    else: print(n,'none')
