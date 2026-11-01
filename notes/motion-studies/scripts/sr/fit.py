import numpy as np
data={
 'create_label_x (F52-60)':[742,694,666,650,640,636,634,634,634],
 'cursor_create_y (F58-67)':[719,641,586,547,520,500,485,476,470,467],
 'cursor_create_x (F58-67)':[894,860,838,823,812,804,798,794,791,790],
 'dashboard_whip_y (F72-79)':[89,100,108,113,116,117,117,117],
 'emoji_resize_w (F200-219)':[108,196,258,304,340,372,398,418,438,454,466,478,488,496,502,508,512,514,518,518],
 'never_converge_h (F224-235)':[289,294,224,166,127,88,76,78,72,70,70,69],
 'pill_morph_w (F462-468)':[230,446,536,582,610,622,622],
 'ticket_layout_x (F462-474)':[556,476,462,450,441,435,430,427,424,422,421,420,420],
 'chart_cam_x (F555-568)':[507,449,409,385,370,361,352,343,335,328,322,315,309,303],
 'so_settle_topy (F251-265)':[414,415,419,424,429,433,437,440,442,445,446,448,449,450,450],
 'zoomthrough_top (F607-615)':[245,236,211,174,132,86,43,7,0],
 'press_w (F602-606)':[741,728,695,655,621],
 'riffle_orange_up (F446-452)':[266,248,236,210,188,180,180],
}
def bez(x1,y1,x2,y2,n=400):
    t=np.linspace(0,1,n); bx=3*(1-t)**2*t*x1+3*(1-t)*t**2*x2+t**3; by=3*(1-t)**2*t*y1+3*(1-t)*t**2*y2+t**3
    return bx,by
cands=[]
for x1 in np.arange(0,1.01,0.1):
  for y1 in np.arange(0,1.6,0.1):
    for x2 in np.arange(0,1.01,0.1):
      for y2 in np.arange(0.6,1.31,0.1):
        cands.append((x1,y1,x2,y2))

C=np.array(cands); xs=np.linspace(0,1,201)
LUT=np.empty((len(C),201))
for i,c in enumerate(C):
    bx,by=bez(*c); LUT[i]=np.interp(xs,bx,by)
def affine_err(Y,v):
    # Y: (C,n) ; fit v ~ a+b*Y per row
    n=v.size; my=Y.mean(1,keepdims=True); mv=v.mean()
    cov=((Y-my)*(v-mv)).sum(1); var=((Y-my)**2).sum(1)+1e-12
    b=cov/var; a=mv-b*my[:,0]
    e=((a[:,None]+b[:,None]*Y-v)**2).sum(1); return e
for k,v in data.items():
    v=np.array(v,float); n=len(v); start,end=v[0],v[-1]; prog=(v-start)/(end-start)
    d=np.abs(np.diff(v))
    ratios=[d[i+1]/d[i] for i in range(len(d)-1) if d[i]>2 and d[i+1]>0.5]
    pk=int(np.argmax(d)); ov=(np.max(prog)-1)*100
    t90=next(i for i,p in enumerate(prog) if p>=0.9)
    settle=next(i for i in range(n) if all(abs(v[j]-end)<=1.5 for j in range(i,n)))
    best=(1e18,None)
    for D in range(max(2,n-3), n+14):
        for s0 in range(0,7):
            ts=np.clip((s0+np.arange(n))/D,0,1); idx=np.round(ts*200).astype(int)
            e=affine_err(LUT[:,idx],v); i=int(np.argmin(e))
            if e[i]<best[0]: best=(e[i],(C[i],D,s0))
    c,D,s0=best[1]; rms=np.sqrt(best[0]/n)
    # exponential fit v=end'+A r^i
    be=(1e18,None)
    for r in np.arange(0.3,0.96,0.01):
        Y=r**np.arange(n); e=affine_err(Y[None,:],v)[0]
        if e<be[0]: be=(e,r)
    print(f"{k}: n={n-1}f peakV@{pk} ({d[pk]:.0f}px/f) t90={t90}f settle={settle}f overshoot={ov:.1f}% medianRatio={np.median(ratios) if ratios else float('nan'):.2f} | expo r={be[1]:.2f} rms={np.sqrt(be[0]/n):.1f} | bezier({c[0]:.1f},{c[1]:.1f},{c[2]:.1f},{c[3]:.1f}) dur={D}f phase={s0}f rms={rms:.1f}px")
