import sys
from PIL import Image
d=sys.argv[1]; a,b=int(sys.argv[2]),int(sys.argv[3]); ymax=int(sys.argv[4])
cols={'coral':(232,85,88),'blue':(58,127,196),'green':(4,76,26)}
for n in range(a,b+1):
    im=Image.open(f"{d}/f{n:03d}.png").convert('RGB'); p=im.load()
    out=[]
    for k,c in cols.items():
        xs=[];ys=[]
        for y in range(0,ymax,1):
            for x in range(300,1000,1):
                q=p[x,y]
                if sum((u-v)**2 for u,v in zip(q,c))<1500: xs.append(x);ys.append(y)
        if xs: out.append(f"{k}: x{min(xs)}-{max(xs)} y{min(ys)}-{max(ys)} w{max(xs)-min(xs)+1} h{max(ys)-min(ys)+1} cy{(min(ys)+max(ys))/2:.0f}")
        else: out.append(f"{k}: -")
    print(n,' | '.join(out))
