import sys
from PIL import Image
d=sys.argv[1]; a,b=int(sys.argv[2]),int(sys.argv[3])
W,H=640,360
for n in range(a,b+1):
    im=Image.open(f"{d}/f{n:03d}.png").convert('RGB').resize((W,H),Image.NEAREST); p=im.load()
    m=bytearray(W*H)
    for y in range(H):
        for x in range(W):
            r,g,bb=p[x,y]
            if g>170 and bb>180 and r<175: m[y*W+x]=1
    seen=bytearray(W*H); hs=[]
    for i in range(W*H):
        if m[i] and not seen[i]:
            st=[i];seen[i]=1;ys=[];xs=[]
            while st:
                j=st.pop();ys.append(j//W);xs.append(j%W);x=j%W
                for k in (j-1,j+1,j-W,j+W):
                    if 0<=k<W*H and m[k] and not seen[k] and abs((k%W)-x)<=1: seen[k]=1;st.append(k)
            if len(ys)>4 and min(ys)>2 and max(ys)<H-3: hs.append(2*(max(ys)-min(ys)+1))
    hs.sort()
    if hs: print(n,'pillars',len(hs),'height p10/p50/p90/max',hs[len(hs)//10],hs[len(hs)//2],hs[9*len(hs)//10],hs[-1])
    else: print(n,'none')
