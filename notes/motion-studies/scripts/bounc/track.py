import sys
W,H=640,360
f=sys.stdin.buffer
prev=None
n=0
print("frame\tt\tarea\tcx\tcy\tx0\tx1\ty0\ty1\tw\th\tmeanY\tdiff\tblobs")
while True:
    b=f.read(W*H*3)
    if len(b)<W*H*3: break
    area=0;sx=0;sy=0;x0=W;x1=-1;y0=H;y1=-1;lum=0;diff=0
    cols=set()
    for y in range(H):
        row=b[y*W*3:(y+1)*W*3]
        for x in range(0,W):
            r=row[3*x];g=row[3*x+1];bb=row[3*x+2]
            if bb>120 and r<90 and g<90 and bb-r>80:
                area+=1;sx+=x;sy+=y
                if x<x0:x0=x
                if x>x1:x1=x
                if y<y0:y0=y
                if y>y1:y1=y
        if y%4==0:
            for x in range(0,W*3,12):
                lum+=row[x+1]
                if prev is not None: diff+=abs(row[x+1]-prev[y*W*3+x+1])
    cnt=(H//4)*(W//4)
    t=n/30
    if area:
        print(f"{n}\t{t:.3f}\t{area}\t{sx/area:.1f}\t{sy/area:.1f}\t{x0}\t{x1}\t{y0}\t{y1}\t{x1-x0+1}\t{y1-y0+1}\t{lum/cnt:.1f}\t{diff/cnt:.2f}")
    else:
        print(f"{n}\t{t:.3f}\t0\t\t\t\t\t\t\t\t\t{lum/cnt:.1f}\t{diff/cnt:.2f}")
    prev=b;n+=1
