import sys
from PIL import Image
sys.setrecursionlimit(100000)
d=sys.argv[1]; a,b=int(sys.argv[2]),int(sys.argv[3]); bg=tuple(int(v) for v in sys.argv[4].split(','))
W,H=640,360
for n in range(a,b+1):
    im=Image.open(f"{d}/f{n:03d}.png").convert('RGB').resize((W,H),Image.NEAREST); p=im.load()
    m=bytearray(W*H)
    for y in range(H):
        for x in range(W):
            q=p[x,y]
            if abs(q[0]-bg[0])+abs(q[1]-bg[1])+abs(q[2]-bg[2])>90: m[y*W+x]=1
    seen=bytearray(W*H); blobs=[]
    for i in range(W*H):
        if m[i] and not seen[i]:
            st=[i];seen[i]=1;pts=[]
            while st:
                j=st.pop();pts.append(j);x=j%W;y=j//W
                for k in (j-1,j+1,j-W,j+W):
                    if 0<=k<W*H and m[k] and not seen[k] and abs((k%W)-x)<=1: seen[k]=1;st.append(k)
            if len(pts)>=3:
                xs=[q%W for q in pts];ys=[q//W for q in pts]
                mx=sum(xs)/len(xs);my=sum(ys)/len(ys)
                sxx=sum((x-mx)**2 for x in xs)/len(xs);syy=sum((y-my)**2 for y in ys)/len(ys);sxy=sum((x-mx)*(y-my) for x,y in zip(xs,ys))/len(xs)
                tr=sxx+syy;det=sxx*syy-sxy*sxy;l1=tr/2+((tr/2)**2-det)**0.5;l2=tr/2-((tr/2)**2-det)**0.5
                blobs.append((len(pts),mx,my,(12*l1)**0.5,(12*max(l2,0))**0.5))
    if not blobs: print(n,'0 blobs');continue
    blobs.sort(key=lambda t:t[0])
    med=blobs[len(blobs)//2]
    L=sorted(t[3] for t in blobs);Wd=sorted(t[4] for t in blobs)
    xs=[t[1] for t in blobs];ys=[t[2] for t in blobs]
    print(n,f"n={len(blobs)} medlen={2*L[len(L)//2]:.1f} medwid={2*Wd[len(Wd)//2]:.1f} (full-res px) extent x{2*min(xs):.0f}-{2*max(xs):.0f} y{2*min(ys):.0f}-{2*max(ys):.0f}")
