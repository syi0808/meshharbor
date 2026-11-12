#define main original_cli_main
#include "../src/refine.cpp"
#undef main
#include <random>
void check(bool ok,const char*msg){if(!ok)throw std::runtime_error(msg);}
V point(double u,double v,double zoffset=0){V p;p.uv={u,v};p.p={1+2*u,-2+3*v,.5*u+v+zoffset};p.n={0,0,1};return p;}
int main(){try{
 std::vector<V> vs{point(0,0),point(1,0),point(1,1),point(0,1)};std::vector<T> fs{{{0,1,2}},{{0,2,3}}};UVEnvelope e(vs,fs);
 auto m=e.lookup({.2,.3});check(m.status==UVEnvelope::VALID&&norm(sub(m.point,point(.2,.3).p))<1e-12,"Affine UV map failed");
 m=e.lookup({.5,.5});check(m.status==UVEnvelope::VALID&&m.hits==2&&m.spread<1e-12,"Shared edge incorrectly ambiguous");
 check(e.lookup({1.1,.5}).status==UVEnvelope::OUTSIDE,"Outside atlas bbox not rejected");
 UVEnvelope half(vs,{fs[0]});check(half.lookup({.1,.9}).status==UVEnvelope::OUTSIDE,"Inside bbox, outside triangle not rejected");
 auto overlapVs=vs;for(auto v:vs){v.p[2]+=1;overlapVs.push_back(v);}auto overlapFs=fs;overlapFs.push_back({{4,5,6}});overlapFs.push_back({{4,6,7}});UVEnvelope overlap(overlapVs,overlapFs);check(overlap.lookup({.2,.3}).status==UVEnvelope::AMBIGUOUS,"Distinct overlapping atlas layers not detected");
 std::array<Vec2,3> uv{{{.1,.1},{.8,.1},{.1,.8}}};std::array<Vec3,3> p;for(int i=0;i<3;i++)p[i]=point(uv[i][0],uv[i][1],.002).p;UVEnvelope::Status st;double worst;check(e.probe_triangle(uv,p,.003,st,worst)&&std::abs(worst-.002)<1e-12,"Low-error candidate should pass");for(auto&v:p)v[2]+=.01;check(!e.probe_triangle(uv,p,.003,st,worst)&&st==UVEnvelope::VALID,"High-error candidate should fail distance guard");
 std::vector<V> degvs{point(0,0),point(0,0),point(0,0)};UVEnvelope deg(degvs,{{{0,1,2}}});check(deg.skipped_singular==1&&deg.lookup({0,0}).status==UVEnvelope::OUTSIDE,"Singular UV triangle not excluded");
 // Original piecewise-linear tent; broad new triangle passes corner check, but midpoint/centroid must reject.
 std::vector<V> tent(4);tent[0].uv={0,0};tent[0].p={0,0,0};tent[1].uv={1,0};tent[1].p={1,0,0};tent[2].uv={0,1};tent[2].p={0,1,0};tent[3].uv={1./3,1./3};tent[3].p={1./3,1./3,1};UVEnvelope tentmap(tent,{{{0,1,3}},{{1,2,3}},{{2,0,3}}});std::array<Vec2,3> tu{{{0,0},{1,0},{0,1}}};std::array<Vec3,3> tp{{{0,0,0},{1,0,0},{0,1,0}}};check(!tentmap.probe_triangle(tu,tp,.1,st,worst)&&st==UVEnvelope::VALID&&worst>.99,"Interior tent damage not rejected");
 // UV differential metric detects bounded directional changes, including anisotropy.
 std::array<Vec2,3> mu{{{0,0},{1,0},{0,1}}};std::array<Vec3,3> mp{{{0,0,0},{1,0,0},{0,1,0}}};auto mr=mapping_metric(mp,mu);mp[1][0]=1.25;mp[2][1]=.8;auto mn=mapping_metric(mp,mu);double slo,shi;check(relative_stretches(mr,mn,slo,shi)&&std::abs(slo-.8)<1e-12&&std::abs(shi-1.25)<1e-12,"Relative UV stretch eigenvalue bound failed");
 Q q=planeq({0,0,1},-1,1);check(std::abs(qem_edge_t(q,{0,0,0},{0,0,2})-.5)<1e-12,"QEM direction homogeneous coordinate regression");q=planeq({0,0,1},-4,1);check(std::abs(qem_edge_t(q,{0,0,3},{0,0,2})-.5)<1e-12,"Translated QEM plane regression");check(qem_edge_t(q,{0,0,5},{0,0,1})==0,"Endpoint clamp failed");
 std::mt19937 rng(4821);std::uniform_real_distribution<double> unif(-2,2);for(int t=0;t<100;t++){Q qq;for(int j=0;j<5;j++){Vec3 n=normed({unif(rng),unif(rng),unif(rng)});qq+=planeq(n,unif(rng),.1+std::abs(unif(rng)));}Vec3 p0{unif(rng),unif(rng),unif(rng)},d{unif(rng),unif(rng),unif(rng)};double x=qem_edge_t(qq,p0,d);auto value=[&](double a){return qeval(qq,{p0[0]+a*d[0],p0[1]+a*d[1],p0[2]+a*d[2]});};for(int j=0;j<=1000;j++)check(value(x)<=value(j/1000.)+1e-10,"QEM solution worse than sampled edge");}
 std::cout<<"PASS: affine mapping; shared edge; outside bbox; outside footprint; ambiguous overlap; accepted low error; rejected high error; excluded singular UV; interior tent; QEM translated/direction/clamp and 100 randomized PSD quadrics.\n";
 return 0;
 }catch(const std::exception&e){std::cerr<<"FAIL "<<e.what()<<"\n";return 1;}}
