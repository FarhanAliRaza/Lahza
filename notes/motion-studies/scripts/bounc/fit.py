import sys,math
rows={}
for l in open(sys.argv[1]).read().split('\n')[1:]:
    p=l.split('\t')
    if len(p)>4 and p[3]: rows[int(p[0])]=(float(p[3]),float(p[4]),int(p[9]),int(p[10]))
def bez(x1,y1,x2,y2,t):
    # solve x(u)=t
    lo,hi=0.0,1.0
    for _ in range(40):
        u=(lo+hi)/2; x=3*(1-u)**2*u*x1+3*(1-u)*u*u*x2+u**3
        if x<t: lo=u
        else: hi=u
    u=(lo+hi)/2; return 3*(1-u)**2*u*y1+3*(1-u)*u*u*y2+u**3
def fitbez(prog, fixed_first=None):
    best=None
    g=[i/20 for i in range(0,21)]
    for x1 in g:
      for y1 in [0]+[i/10 for i in range(1,21)]:
        for x2 in g:
          for y2 in [1.0]:
            e=sum((bez(x1,y1,x2,y2,t)-v)**2 for t,v in prog)
            if best is None or e<best[0]: best=(e,x1,y1,x2,y2)
    return best
def report(name,f0,f1,axis):
    vals=[(f,rows[f][axis]) for f in range(f0,f1+1) if f in rows]
    a=vals[0][1]; b=vals[-1][1]; n=f1-f0
    prog=[((f-f0)/n,(v-a)/(b-a)) for f,v in vals]
    vel=[round((vals[i+1][1]-vals[i][1])/(b-a)*n,2) for i in range(len(vals)-1)]
    pk=max(range(len(vel)),key=lambda i:abs(vel[i]))
    bb=fitbez(prog)
    # power fits
    pin=min(((sum((t**p-v)**2 for t,v in prog),p) for p in [x/20 for x in range(10,121)]))
    pout=min(((sum((1-(1-t)**p-v)**2 for t,v in prog),p) for p in [x/20 for x in range(10,121)]))
    # expo out with k
    eo=min(((sum(((1-math.exp(-k*t))/(1-math.exp(-k))-v)**2 for t,v in prog),k) for k in [x/4 for x in range(1,81)]))
    ei=min(((sum(((math.exp(k*t)-1)/(math.exp(k)-1)-v)**2 for t,v in prog),k) for k in [x/4 for x in range(1,81)]))
    print(f"\n## {name}: frames {f0}-{f1} ({n} f), axis {'xy'[axis]}, {a:.1f}->{b:.1f} (640-scale px)")
    print(" progress:", ' '.join(f"{v:.3f}" for t,v in prog))
    print(" norm velocity (1=linear avg):", vel, "peak at step", pk)
    print(f" best cubic-bezier({bb[1]},{bb[2]},{bb[3]},{bb[4]}) rms={math.sqrt(bb[0]/len(prog)):.3f}")
    print(f" power-in t^{pin[1]} rms={math.sqrt(pin[0]/len(prog)):.3f}; power-out 1-(1-t)^{pout[1]} rms={math.sqrt(pout[0]/len(prog)):.3f}")
    print(f" expo-out k={eo[1]} rms={math.sqrt(eo[0]/len(prog)):.3f}; expo-in k={ei[1]} rms={math.sqrt(ei[0]/len(prog)):.3f}")
report("S1 opening drop",6,12,1)
report("S1 rebound rise",14,30,1)
report("S1 fall to contact",30,47,1)
report("S1 roll-off accel (x)",150,198,0)
report("S2 launch rise",282,294,1)
report("S2 arc fall",294,307,1)
report("S2 arc horizontal",282,307,0)
report("S8 entrance decel (x)",538,565,0)
report("S8 drop off edge",565,571,1)
report("S3 rebound rise",386,391,1)
