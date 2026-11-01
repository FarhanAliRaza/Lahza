import math
def bez(x1,y1,x2,y2,t):
    # solve x(s)=t
    lo,hi=0.0,1.0
    for _ in range(40):
        s=(lo+hi)/2; x=3*(1-s)**2*s*x1+3*(1-s)*s*s*x2+s**3
        if x<t: lo=s
        else: hi=s
    s=(lo+hi)/2; return 3*(1-s)**2*s*y1+3*(1-s)*s*s*y2+s**3
def fit(name,vals,log=False):
    v=[math.log(x) for x in vals] if log else vals
    a,b=v[0],v[-1]; p=[(x-a)/(b-a) for x in v]; n=len(p)-1
    t=[i/n for i in range(n+1)]
    best=None
    g=[i/20 for i in range(21)]
    for x1 in g:
      for y1 in [i/10 for i in range(0,21)]:
        for x2 in g:
          for y2 in [i/10 for i in range(0,16)]:
            e=sum((bez(x1,y1,x2,y2,ti)-pi)**2 for ti,pi in zip(t,p))
            if best is None or e<best[0]: best=(e,x1,y1,x2,y2)
    vel=[p[i+1]-p[i] for i in range(n)]
    pk=max(range(n),key=lambda i:vel[i])
    t90=next(i for i in range(n+1) if p[i]>=0.9)
    print(f'{name}: frames={n} t90={t90}f peakvel at step {pk+1} ({vel[pk]:.2f}/f) max p={max(p):.3f}')
    print('  progress',' '.join(f'{x:.2f}' for x in p))
    print('  bezier(%.2f,%.2f,%.2f,%.2f) rmse %.3f'%(best[1],best[2],best[3],best[4],math.sqrt(best[0]/(n+1))))
fit('pink pedestal push (bbox h, f97-112)',[84,108,136,166,190,206,220,230,240,248,254,260,262,266,268,270])
fit('Licensed rise (cy, f668-679)',[154.6,120.8,89.8,76.9,71.0,66.5,63.2,61.1,59.1,57.9,56.8,56.2])
fit('Licensed rise incl. assumed f667 start 190',[190,154.6,120.8,89.8,76.9,71.0,66.5,63.2,61.1,59.1,57.9,56.8,56.2])
fit('templates zoom (log h, f219-224)',[34,70,126,184,196,206],log=True)
fit('ma logo fly-in x (f927-934)',[223.6,197.8,175.4,150.1,136.5,127.3,122.8,121.9])
fit('Best grow (log w, f866-876)',[46,48,58,78,100,138,210,280,320,342,358],log=True)
fit('Fully re-centre F x (f655-661)',[414.4,372.2,314.9,253.1,215.0,192.9,190])
