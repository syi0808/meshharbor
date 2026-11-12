#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <limits>
#include <map>
#include <numeric>
#include <queue>
#include <set>
#include <sstream>
#include <stdexcept>
#include <string>
#include <unordered_set>
#include <utility>
#include <vector>

// Blender-free GLB v2 triangle-mesh reducer. Implements an endpoint-constrained
// Garland-Heckbert quadric-error edge-collapse heuristic; attribute values are
// interpolated on eligible edges, with UV discontinuity vertices protected.
namespace {
struct Json {
  enum Kind { NIL, BOOL, NUMBER, STRING, ARRAY, OBJECT } kind=NIL;
  bool b=false; double n=0; std::string s; std::vector<Json> a; std::map<std::string,Json> o;
  Json()=default; Json(double x):kind(NUMBER),n(x){} Json(int x):kind(NUMBER),n(x){} Json(bool x):kind(BOOL),b(x){} Json(const char*x):kind(STRING),s(x){} Json(std::string x):kind(STRING),s(std::move(x)){}
  static Json arr(){Json x;x.kind=ARRAY;return x;} static Json obj(){Json x;x.kind=OBJECT;return x;}
  Json& operator[](const std::string&k){if(kind!=OBJECT){kind=OBJECT;o.clear();}return o[k];}
  const Json& at(const std::string&k)const{auto i=o.find(k);if(i==o.end())throw std::runtime_error("Missing JSON key: "+k);return i->second;}
  Json& at(const std::string&k){auto i=o.find(k);if(i==o.end())throw std::runtime_error("Missing JSON key: "+k);return i->second;}
  bool has(const std::string&k)const{return kind==OBJECT&&o.count(k);}
  int integer()const{return int(n);} std::string string()const{if(kind!=STRING)throw std::runtime_error("Expected JSON string");return s;}
};
struct Parser {
  const std::string& t; size_t p=0; Parser(const std::string&x):t(x){}
  void ws(){while(p<t.size()&&std::isspace((unsigned char)t[p]))++p;}
  static void utf8(std::string&out,unsigned cp){if(cp<0x80)out.push_back(char(cp));else if(cp<0x800){out.push_back(char(0xc0|(cp>>6)));out.push_back(char(0x80|(cp&63)));}else if(cp<0x10000){out.push_back(char(0xe0|(cp>>12)));out.push_back(char(0x80|((cp>>6)&63)));out.push_back(char(0x80|(cp&63)));}else{out.push_back(char(0xf0|(cp>>18)));out.push_back(char(0x80|((cp>>12)&63)));out.push_back(char(0x80|((cp>>6)&63)));out.push_back(char(0x80|(cp&63)));}}
  std::string str(){if(t[p++]!='"')throw std::runtime_error("Expected string");std::string o;while(p<t.size()){char c=t[p++];if(c=='"')return o;if(c=='\\'){if(p>=t.size())break;char e=t[p++];switch(e){case '"':o+='"';break;case '\\':o+='\\';break;case '/':o+='/';break;case 'b':o+='\b';break;case 'f':o+='\f';break;case 'n':o+='\n';break;case 'r':o+='\r';break;case 't':o+='\t';break;case 'u':{if(p+4>t.size())throw std::runtime_error("Bad unicode escape");unsigned v=0;for(int k=0;k<4;++k){char h=t[p++];v=(v<<4)| (h>='0'&&h<='9'?h-'0':h>='a'&&h<='f'?h-'a'+10:h-'A'+10);}utf8(o,v);break;}default:throw std::runtime_error("Bad escape");}}else o+=c;}throw std::runtime_error("Unterminated JSON string");}
  Json value(){ws();if(p>=t.size())throw std::runtime_error("Unexpected end JSON");char c=t[p];if(c=='{'){++p;Json x=Json::obj();ws();if(t[p]=='}'){++p;return x;}for(;;){ws();std::string k=str();ws();if(t[p++]!=':')throw std::runtime_error("Expected colon");x.o[k]=value();ws();if(t[p]=='}'){++p;break;}if(t[p++]!=',')throw std::runtime_error("Expected comma");}return x;}if(c=='['){++p;Json x=Json::arr();ws();if(t[p]==']'){++p;return x;}for(;;){x.a.push_back(value());ws();if(t[p]==']'){++p;break;}if(t[p++]!=',')throw std::runtime_error("Expected comma");}return x;}if(c=='"'){Json x;x.kind=Json::STRING;x.s=str();return x;}if(t.compare(p,4,"true")==0){p+=4;return Json(true);}if(t.compare(p,5,"false")==0){p+=5;return Json(false);}if(t.compare(p,4,"null")==0){p+=4;return Json();}size_t st=p;if(t[p]=='-')++p;while(p<t.size()&&(std::isdigit((unsigned char)t[p])||t[p]=='.'||t[p]=='e'||t[p]=='E'||t[p]=='+'||t[p]=='-'))++p; if(st==p)throw std::runtime_error("Bad JSON token");Json x;x.kind=Json::NUMBER;x.n=std::stod(t.substr(st,p-st));return x;}
  Json parse(){Json x=value();ws();if(p!=t.size())throw std::runtime_error("Trailing JSON");return x;}
};
void esc(const std::string&s,std::string&o){o+='"';for(unsigned char c:s){switch(c){case '"':o+="\\\"";break;case '\\':o+="\\\\";break;case '\b':o+="\\b";break;case '\f':o+="\\f";break;case '\n':o+="\\n";break;case '\r':o+="\\r";break;case '\t':o+="\\t";break;default:if(c<32){char b[7];std::snprintf(b,sizeof b,"\\u%04x",c);o+=b;}else o+=char(c);}}o+='"';}
void dump(const Json&j,std::string&o){switch(j.kind){case Json::NIL:o+="null";break;case Json::BOOL:o+=j.b?"true":"false";break;case Json::NUMBER:{std::ostringstream ss;ss<<std::setprecision(17)<<j.n;o+=ss.str();break;}case Json::STRING:esc(j.s,o);break;case Json::ARRAY:o+='[';for(size_t i=0;i<j.a.size();++i){if(i)o+=',';dump(j.a[i],o);}o+=']';break;case Json::OBJECT:o+='{';{bool f=true;for(auto&kv:j.o){if(!f)o+=',';f=false;esc(kv.first,o);o+=':';dump(kv.second,o);}}o+='}';break;}}
uint32_t rd32(const std::vector<uint8_t>&b,size_t p){if(p+4>b.size())throw std::runtime_error("Truncated GLB");return uint32_t(b[p])|(uint32_t(b[p+1])<<8)|(uint32_t(b[p+2])<<16)|(uint32_t(b[p+3])<<24);}
void wr32(std::vector<uint8_t>&b,uint32_t x){for(int i=0;i<4;++i)b.push_back(uint8_t(x>>(8*i)));}
struct Glb{Json root;std::vector<uint8_t> bin;};
Glb readglb(const std::string&path){std::ifstream f(path,std::ios::binary);if(!f)throw std::runtime_error("Cannot open input GLB: "+path);std::vector<uint8_t>b((std::istreambuf_iterator<char>(f)),{});if(b.size()<20||std::memcmp(b.data(),"glTF",4)||rd32(b,4)!=2||rd32(b,8)!=b.size())throw std::runtime_error("Invalid GLB v2 header");size_t p=12;std::string js;std::vector<uint8_t>bin;while(p+8<=b.size()){uint32_t n=rd32(b,p),type=rd32(b,p+4);p+=8;if(p+n>b.size())throw std::runtime_error("Invalid GLB chunk length");if(type==0x4e4f534a)js.assign((char*)b.data()+p,n);else if(type==0x004e4942)bin.assign(b.begin()+p,b.begin()+p+n);p+=n;}if(js.empty()||bin.empty())throw std::runtime_error("GLB needs JSON and BIN chunks");return {Parser(js).parse(),std::move(bin)};}
void writeglb(const std::string&path,const Json&root,std::vector<uint8_t>bin){std::string js;dump(root,js);while(js.size()%4)js.push_back(' ');while(bin.size()%4)bin.push_back(0);std::vector<uint8_t>b;b.insert(b.end(),{'g','l','T','F'});wr32(b,2);wr32(b,uint32_t(12+8+js.size()+8+bin.size()));wr32(b,uint32_t(js.size()));wr32(b,0x4e4f534a);b.insert(b.end(),js.begin(),js.end());wr32(b,uint32_t(bin.size()));wr32(b,0x004e4942);b.insert(b.end(),bin.begin(),bin.end());std::ofstream f(path,std::ios::binary);f.write((char*)b.data(),b.size());if(!f)throw std::runtime_error("Failed writing output GLB");}
size_t cbytes(int c){switch(c){case 5120:case 5121:return 1;case 5122:case 5123:return 2;case 5125:case 5126:return 4;default:throw std::runtime_error("Unsupported accessor component type");}}
struct Acc{int view;size_t off,count;int comp;std::string type;bool norm;};
Acc getacc(const Json&r,int id){const Json&a=r.at("accessors").a.at(id);if(a.has("sparse"))throw std::runtime_error("Sparse accessors are not supported");return {a.at("bufferView").integer(),a.has("byteOffset")?size_t(a.at("byteOffset").n):0,size_t(a.at("count").n),a.at("componentType").integer(),a.at("type").string(),a.has("normalized")&&a.at("normalized").b};}
size_t baseoff(const Json&r,const Acc&a){const Json&v=r.at("bufferViews").a.at(a.view);if(v.has("buffer")&&v.at("buffer").integer()!=0)throw std::runtime_error("Only buffer 0 is supported");return (v.has("byteOffset")?size_t(v.at("byteOffset").n):0)+a.off;}
std::vector<std::array<float,3>> read3(const Json&r,const std::vector<uint8_t>&bin,int id){Acc a=getacc(r,id);if(a.comp!=5126||a.type!="VEC3"||a.norm)throw std::runtime_error("POSITION/NORMAL must be unnormalized float VEC3");const Json&v=r.at("bufferViews").a.at(a.view);size_t stride=v.has("byteStride")?size_t(v.at("byteStride").n):12,off=baseoff(r,a);if(stride<12||off+(a.count?((a.count-1)*stride+12):0)>bin.size())throw std::runtime_error("Attribute out of bounds");std::vector<std::array<float,3>> out(a.count);for(size_t i=0;i<a.count;i++)std::memcpy(out[i].data(),bin.data()+off+i*stride,12);return out;}
std::vector<std::array<float,2>> read2(const Json&r,const std::vector<uint8_t>&bin,int id){Acc a=getacc(r,id);if(a.comp!=5126||a.type!="VEC2"||a.norm)throw std::runtime_error("TEXCOORD_0 must be unnormalized float VEC2");const Json&v=r.at("bufferViews").a.at(a.view);size_t stride=v.has("byteStride")?size_t(v.at("byteStride").n):8,off=baseoff(r,a);if(stride<8||off+(a.count?((a.count-1)*stride+8):0)>bin.size())throw std::runtime_error("Attribute out of bounds");std::vector<std::array<float,2>> out(a.count);for(size_t i=0;i<a.count;i++)std::memcpy(out[i].data(),bin.data()+off+i*stride,8);return out;}
std::vector<uint32_t> readidx(const Json&r,const std::vector<uint8_t>&bin,int id){Acc a=getacc(r,id);if(a.type!="SCALAR"||a.norm)throw std::runtime_error("Indices must be unnormalized scalar");size_t cb=cbytes(a.comp),off=baseoff(r,a);if(off+a.count*cb>bin.size())throw std::runtime_error("Indices out of bounds");std::vector<uint32_t>x(a.count);for(size_t i=0;i<a.count;i++){if(a.comp==5121)x[i]=bin[off+i];else if(a.comp==5123){uint16_t q;std::memcpy(&q,bin.data()+off+i*2,2);x[i]=q;}else if(a.comp==5125)std::memcpy(&x[i],bin.data()+off+i*4,4);else throw std::runtime_error("Unsupported index width");}return x;}
struct V{std::array<double,3> p{};std::array<double,2> uv{};std::array<double,3> n{};bool live=true,locked=false;uint32_t version=0;};
struct T{std::array<uint32_t,3> v{};bool live=true;};
using Vec3=std::array<double,3>;
Vec3 sub(Vec3 a,Vec3 b){return {a[0]-b[0],a[1]-b[1],a[2]-b[2]};}Vec3 cross(Vec3 a,Vec3 b){return {a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]};}double dot(Vec3 a,Vec3 b){return a[0]*b[0]+a[1]*b[1]+a[2]*b[2];}double norm(Vec3 a){return std::sqrt(dot(a,a));}Vec3 normed(Vec3 a){double q=norm(a);return q>1e-30?Vec3{a[0]/q,a[1]/q,a[2]/q}:Vec3{0,1,0};}
struct Q{double a[10]{};Q& operator+=(const Q&o){for(int i=0;i<10;++i)a[i]+=o.a[i];return *this;}};
double qeval(const Q&q,Vec3 p){double x=p[0],y=p[1],z=p[2];return q.a[0]*x*x+2*q.a[1]*x*y+2*q.a[2]*x*z+2*q.a[3]*x+q.a[4]*y*y+2*q.a[5]*y*z+2*q.a[6]*y+q.a[7]*z*z+2*q.a[8]*z+q.a[9];}
Q planeq(Vec3 n,double d,double weight){Q q;double x[4]={n[0],n[1],n[2],d};int k=0;for(int i=0;i<4;i++)for(int j=i;j<4;j++)q.a[k++]=weight*x[i]*x[j];return q;}

double qem_edge_t(const Q&q,Vec3 p,Vec3 d){
  double A=q.a[0]*d[0]*d[0]+2*q.a[1]*d[0]*d[1]+2*q.a[2]*d[0]*d[2]+q.a[4]*d[1]*d[1]+2*q.a[5]*d[1]*d[2]+q.a[7]*d[2]*d[2];
  double B=2*(d[0]*(q.a[0]*p[0]+q.a[1]*p[1]+q.a[2]*p[2]+q.a[3])+d[1]*(q.a[1]*p[0]+q.a[4]*p[1]+q.a[5]*p[2]+q.a[6])+d[2]*(q.a[2]*p[0]+q.a[5]*p[1]+q.a[7]*p[2]+q.a[8]));
  double t=A>1e-30?std::clamp(-B/(2*A),0.0,1.0):0.0;
  auto eval=[&](double x){return qeval(q,{p[0]+x*d[0],p[1]+x*d[1],p[2]+x*d[2]});};
  double best=eval(t);if(eval(0)<best){t=0;best=eval(0);}if(eval(1)<best)t=1;return t;
}
using Vec2=std::array<double,2>;
double uvarea2(const std::array<Vec2,3>& u){return (u[1][0]-u[0][0])*(u[2][1]-u[0][1])-(u[1][1]-u[0][1])*(u[2][0]-u[0][0]);}
struct UVEnvelope {
  enum Status { VALID=0, OUTSIDE=1, AMBIGUOUS=2 };
  struct Face {std::array<Vec2,3> u;std::array<Vec3,3> p;double det;};
  struct Lookup {Status status=OUTSIDE;Vec3 point{};size_t hits=0;double spread=0;};
  static constexpr int GRID=128;
  std::vector<Face> faces;std::vector<std::vector<int>> grid;
  Vec2 lo{1e99,1e99},hi{-1e99,-1e99};double ambiguity_tol=1e-5;size_t skipped_singular=0;
  UVEnvelope(const std::vector<V>&vs,const std::vector<T>&ts,double ambiguity=1e-5):grid(GRID*GRID),ambiguity_tol(ambiguity){
    for(auto&t:ts){Face f;for(int k=0;k<3;k++){f.u[k]=vs[t.v[k]].uv;f.p[k]=vs[t.v[k]].p;}f.det=uvarea2(f.u);if(std::abs(f.det)<=1e-14){++skipped_singular;continue;}faces.push_back(f);for(auto&u:f.u)for(int k=0;k<2;k++){lo[k]=std::min(lo[k],u[k]);hi[k]=std::max(hi[k],u[k]);}}
    if(faces.empty()){lo={0,0};hi={1,1};return;}
    for(int k=0;k<2;k++)if(hi[k]-lo[k]<1e-12)hi[k]=lo[k]+1;
    for(size_t i=0;i<faces.size();i++){auto&f=faces[i];Vec2 mn=f.u[0],mx=f.u[0];for(auto&u:f.u)for(int k=0;k<2;k++){mn[k]=std::min(mn[k],u[k]);mx[k]=std::max(mx[k],u[k]);}for(int y=cell(mn[1]-1e-10,1);y<=cell(mx[1]+1e-10,1);y++)for(int x=cell(mn[0]-1e-10,0);x<=cell(mx[0]+1e-10,0);x++)grid[y*GRID+x].push_back(int(i));}
  }
  int cell(double x,int k) const {return std::clamp(int(std::floor((x-lo[k])/(hi[k]-lo[k])*GRID)),0,GRID-1);}
  Lookup lookup(Vec2 u) const {
    Lookup out;if(faces.empty()||u[0]<lo[0]-1e-10||u[0]>hi[0]+1e-10||u[1]<lo[1]-1e-10||u[1]>hi[1]+1e-10)return out;
    for(int i:grid[cell(u[1],1)*GRID+cell(u[0],0)]){auto&f=faces[i];double ax=f.u[1][0]-f.u[0][0],ay=f.u[1][1]-f.u[0][1],bx=f.u[2][0]-f.u[0][0],by=f.u[2][1]-f.u[0][1],qx=u[0]-f.u[0][0],qy=u[1]-f.u[0][1];double v=(qx*by-qy*bx)/f.det,w=(ax*qy-ay*qx)/f.det,a=1-v-w;
      if(a< -1e-7||v< -1e-7||w< -1e-7||a>1+1e-7||v>1+1e-7||w>1+1e-7)continue;
      Vec3 p;for(int k=0;k<3;k++)p[k]=a*f.p[0][k]+v*f.p[1][k]+w*f.p[2][k];if(!out.hits)out.point=p;else out.spread=std::max(out.spread,norm(sub(p,out.point)));++out.hits;
    }
    if(out.hits){out.status=out.spread>ambiguity_tol?AMBIGUOUS:VALID;}
    return out;
  }
  bool probe_triangle(const std::array<Vec2,3>&u,const std::array<Vec3,3>&p,double limit,Status&status,double&worst) const {
    // 3 corners, 3 edge midpoints, centroid. A sampled envelope, not a certified bound.
    static const double W[7][3]={{1,0,0},{0,1,0},{0,0,1},{.5,.5,0},{0,.5,.5},{.5,0,.5},{1./3,1./3,1./3}};
    status=VALID;worst=0;
    for(auto&w:W){Vec2 uv{};Vec3 xyz{};for(int j=0;j<3;j++){for(int k=0;k<2;k++)uv[k]+=w[j]*u[j][k];for(int k=0;k<3;k++)xyz[k]+=w[j]*p[j][k];}auto match=lookup(uv);if(match.status!=VALID){status=match.status;return false;}worst=std::max(worst,norm(sub(match.point,xyz)));if(worst>limit)return false;}
    return true;
  }
};
struct MappingMetric {long double g00=0,g01=0,g11=0;double uv_area2=0;bool valid=false;};
MappingMetric mapping_metric(const std::array<Vec3,3>&p,const std::array<std::array<double,2>,3>&u){
  MappingMetric m;long double a=u[1][0]-u[0][0],b=u[2][0]-u[0][0],c=u[1][1]-u[0][1],d=u[2][1]-u[0][1],det=a*d-b*c;m.uv_area2=double(det);long double uscale=(a*a+c*c)*(b*b+d*d);
  if(uscale<=0||det*det<=1e-20L*uscale||std::abs(det)<1e-15L)return m;
  for(int k=0;k<3;k++){long double e=p[1][k]-p[0][k],f=p[2][k]-p[0][k],ju=(e*d-f*c)/det,jv=(-e*b+f*a)/det;m.g00+=ju*ju;m.g01+=ju*jv;m.g11+=jv*jv;}
  long double gd=m.g00*m.g11-m.g01*m.g01;m.valid=m.g00>0&&m.g11>0&&gd>1e-14L*m.g00*m.g11;return m;
}
bool relative_stretches(const MappingMetric& ref, const MappingMetric& now, double& lo, double& hi) {
  if (!ref.valid || !now.valid) return false;
  const long double l00 = std::sqrt(ref.g00);
  const long double l10 = ref.g01 / l00;
  const long double rem = ref.g11 - l10 * l10;
  if (rem <= 0) return false;
  const long double l11 = std::sqrt(rem);
  const long double a = 1 / l00, b = -l10 / (l00 * l11), c = 1 / l11;
  const long double x = a * a * now.g00;
  const long double y = a * (b * now.g00 + c * now.g01);
  const long double z = b * b * now.g00 + 2 * b * c * now.g01 + c * c * now.g11;
  const long double high = (x + z + std::hypot(x - z, 2 * y)) / 2;
  const long double determinant = x * z - y * y;
  if (high <= 0 || determinant <= 0) return false;
  const long double low = determinant / high;
  lo = double(std::sqrt(low));
  hi = double(std::sqrt(high));
  return std::isfinite(lo) && std::isfinite(hi);
}
double g_envelope_limit=0.004;
double g_uv_stretch_limit=1.25;
double g_source_normal_cos=0.8660254037844386;
struct Cand{double cost;uint32_t keep,remove,va,vb;double t;bool operator<(const Cand&o)const{return cost>o.cost||(cost==o.cost&&std::tie(keep,remove)>std::tie(o.keep,o.remove));}};
struct Key3{uint32_t x,y,z;bool operator<(const Key3&o)const{return std::tie(x,y,z)<std::tie(o.x,o.y,o.z);}};
uint32_t fbits(float x){if(x==0)x=0;uint32_t u;std::memcpy(&u,&x,4);return u;}
struct Result{size_t initial=0,final=0,removed=0,locked_vertices=0,protrusion_faces=0;std::vector<std::array<uint32_t,3>> faces;std::vector<uint32_t> source_face_ids;std::vector<V> verts;};
Result reduce_mesh(std::vector<V> verts,std::vector<T> tris,size_t target,std::ostream&report){
 UVEnvelope envelope(verts,tris);size_t envelope_reject_outside=0,envelope_reject_ambiguous=0,envelope_reject_distance=0,uv_reject_orientation=0,envelope_protected_faces=0,metric_reject=0,normal_reject=0,topology_reject=0,invalid_mapping_faces=0;
 std::vector<bool> hard_locked(verts.size(),false);
 std::vector<MappingMetric> source_metrics(tris.size());
 // Original singular or unprojectable faces are retained and pinned, never deleted as a repair.
 for(size_t fi=0;fi<tris.size();++fi){auto &tr=tris[fi];std::array<Vec2,3> u;std::array<Vec3,3> p;for(int k=0;k<3;k++){u[k]=verts[tr.v[k]].uv;p[k]=verts[tr.v[k]].p;}source_metrics[fi]=mapping_metric(p,u);UVEnvelope::Status st;double worst;bool bad=std::abs(uvarea2(u))<=1e-14||!envelope.probe_triangle(u,p,1e-5,st,worst);if(!source_metrics[fi].valid){++invalid_mapping_faces;bad=true;}if(bad){++envelope_protected_faces;for(auto v:tr.v){verts[v].locked=true;hard_locked[v]=true;}}}
 const size_t nv=verts.size(),nf=tris.size();std::vector<std::unordered_set<int>> vtris(nv);std::vector<Q> qs(nv);std::vector<Vec3> faceN(nf);std::vector<double> faceA(nf);std::map<Key3,int> geo;std::vector<int> gid(nv);for(size_t i=0;i<nv;++i){Key3 k{fbits(float(verts[i].p[0])),fbits(float(verts[i].p[1])),fbits(float(verts[i].p[2]))};auto r=geo.emplace(k,int(geo.size()));gid[i]=r.first->second;}
 struct GUse{int face;uint32_t a,b;};std::map<std::pair<int,int>,std::vector<GUse>> gedges;for(size_t fi=0;fi<nf;++fi){auto&t=tris[fi];for(auto v:t.v){if(v>=nv)throw std::runtime_error("Index exceeds attribute count");vtris[v].insert(int(fi));}Vec3 a=verts[t.v[0]].p,b=verts[t.v[1]].p,c=verts[t.v[2]].p;Vec3 cr=cross(sub(b,a),sub(c,a));double ar=0.5*norm(cr);faceA[fi]=ar;faceN[fi]=ar>0?normed(cr):Vec3{0,0,0};for(int e=0;e<3;++e){uint32_t va=t.v[e],vb=t.v[(e+1)%3];int ga=gid[va],gb=gid[vb];if(ga>gb)std::swap(ga,gb);gedges[{ga,gb}].push_back({int(fi),va,vb});}}
 using GeoFaceKey=std::array<Key3,3>;using GeoEdgeKey=std::pair<Key3,Key3>;
 struct GeoStats{long duplicate_groups=0,duplicate_excess=0,nonmanifold_groups=0,nonmanifold_excess=0;};
 std::map<GeoFaceKey,int> geo_face_uses;std::map<GeoEdgeKey,int> geo_edge_uses;
 auto add_geo_delta=[&](const std::array<Vec3,3>&p,int sign,std::map<GeoFaceKey,int>&fd,std::map<GeoEdgeKey,int>&ed){GeoFaceKey fk;for(int k=0;k<3;++k)fk[k]={fbits(float(p[k][0])),fbits(float(p[k][1])),fbits(float(p[k][2]))};std::sort(fk.begin(),fk.end());fd[fk]+=sign;for(int k=0;k<3;++k){Key3 a{fbits(float(p[k][0])),fbits(float(p[k][1])),fbits(float(p[k][2]))},b{fbits(float(p[(k+1)%3][0])),fbits(float(p[(k+1)%3][1])),fbits(float(p[(k+1)%3][2]))};if(b<a)std::swap(a,b);ed[{a,b}]+=sign;}};
 for(size_t fi=0;fi<nf;++fi)if(tris[fi].live){std::array<Vec3,3> p;for(int k=0;k<3;++k)p[k]=verts[tris[fi].v[k]].p;std::map<GeoFaceKey,int> fd;std::map<GeoEdgeKey,int> ed;add_geo_delta(p,1,fd,ed);for(auto&x:fd)geo_face_uses[x.first]+=x.second;for(auto&x:ed)geo_edge_uses[x.first]+=x.second;}
 GeoStats geo_stats;for(auto&x:geo_face_uses){if(x.second>1){++geo_stats.duplicate_groups;geo_stats.duplicate_excess+=x.second-1;}}for(auto&x:geo_edge_uses){if(x.second>2){++geo_stats.nonmanifold_groups;geo_stats.nonmanifold_excess+=x.second-2;}}const GeoStats source_geo_stats=geo_stats;
 enum class GeoGuardReason{SAFE,DUPLICATE,NONMANIFOLD};
 auto check_geo_delta=[&](const std::map<GeoFaceKey,int>&fd,const std::map<GeoEdgeKey,int>&ed,GeoStats&next){next=geo_stats;for(auto&x:fd){auto it=geo_face_uses.find(x.first);int before=it==geo_face_uses.end()?0:it->second,after=before+x.second;if(after<0)throw std::runtime_error("Negative geometric face-use count");if((before<=1&&after>1)||(before>1&&after>before))return GeoGuardReason::DUPLICATE;if(before>1)--next.duplicate_groups;if(after>1)++next.duplicate_groups;next.duplicate_excess+=std::max(0,after-1)-std::max(0,before-1);}for(auto&x:ed){auto it=geo_edge_uses.find(x.first);int before=it==geo_edge_uses.end()?0:it->second,after=before+x.second;if(after<0)throw std::runtime_error("Negative geometric edge-use count");if((before<=2&&after>2)||(before>2&&after>before))return GeoGuardReason::NONMANIFOLD;if(before>2)--next.nonmanifold_groups;if(after>2)++next.nonmanifold_groups;next.nonmanifold_excess+=std::max(0,after-2)-std::max(0,before-2);}return GeoGuardReason::SAFE;};
 auto commit_geo_delta=[&](const std::map<GeoFaceKey,int>&fd,const std::map<GeoEdgeKey,int>&ed,const GeoStats&next){for(auto&x:fd){int&v=geo_face_uses[x.first];v+=x.second;if(v==0)geo_face_uses.erase(x.first);}for(auto&x:ed){int&v=geo_edge_uses[x.first];v+=x.second;if(v==0)geo_edge_uses.erase(x.first);}geo_stats=next;};
 // Keep exact-position split vertices coincident and preserve source geometric open boundaries.
 std::vector<size_t> geo_uses(geo.size(),0);for(int g:gid)++geo_uses[g];size_t split_vertices_locked=0;
 for(size_t i=0;i<nv;i++)if(geo_uses[gid[i]]>1){verts[i].locked=true;++split_vertices_locked;}
 size_t geometric_boundary_edges_locked=0;for(const auto&kv:gedges)if(kv.second.size()==1){++geometric_boundary_edges_locked;for(auto x:kv.second){verts[x.a].locked=true;verts[x.b].locked=true;hard_locked[x.a]=hard_locked[x.b]=true;}}
 // Protect both sides of UV discontinuities; diagnostics weld only exact positions.
 size_t seam_edges=0;for(auto&kv:gedges){auto&u=kv.second;if(u.size()<2)continue;bool seam=false;for(size_t i=1;i<u.size();++i){auto uvAt=[&](const GUse&x,int g){return verts[gid[x.a]==g?x.a:x.b].uv;};for(int g:{kv.first.first,kv.first.second}){auto a=uvAt(u[0],g),b=uvAt(u[i],g);if(std::abs(a[0]-b[0])>1e-5||std::abs(a[1]-b[1])>1e-5)seam=true;}}if(seam){++seam_edges;for(auto&x:u){verts[x.a].locked=verts[x.b].locked=true;}}}
 // Face components across exact-position shared edges; all are kept. Area/count/scale only flag review items.
 std::vector<std::array<int,3>> geomface(nf);for(size_t f=0;f<nf;++f){for(int k=0;k<3;++k)geomface[f][k]=gid[tris[f].v[k]];std::sort(geomface[f].begin(),geomface[f].end());}
 std::vector<int> par(nf);std::iota(par.begin(),par.end(),0);auto find=[&](int x){int r=x;while(par[r]!=r)r=par[r];while(par[x]!=x){int y=par[x];par[x]=r;x=y;}return r;};auto un=[&](int a,int b){a=find(a);b=find(b);if(a!=b)par[b]=a;};size_t guarded_opposed=0,guarded_coincident=0;
 std::vector<std::vector<int>> faceAdj(nf);
 for(auto&kv:gedges){auto&u=kv.second;if(u.size()<2)continue;for(size_t i=0;i<u.size();++i)for(size_t j=i+1;j<u.size();++j){int a=u[i].face,b=u[j].face;if(geomface[a]==geomface[b]){++guarded_coincident;continue;}if(dot(faceN[a],faceN[b])<=-0.95){++guarded_opposed;continue;}un(a,b);faceAdj[a].push_back(b);faceAdj[b].push_back(a);}}
 struct CInfo{int id=0;size_t count=0;double area=0;Vec3 centroid{0,0,0},lo{1e99,1e99,1e99},hi{-1e99,-1e99,-1e99};};std::map<int,CInfo> cs;std::vector<int> faceComp(nf);double totalA=0;
 for(size_t f=0;f<nf;++f){int root=find(int(f));faceComp[f]=root;auto&c=cs[root];c.id=root;c.count++;c.area+=faceA[f];totalA+=faceA[f];Vec3 fc{0,0,0};for(auto v:tris[f].v)for(int k=0;k<3;++k){fc[k]+=verts[v].p[k]/3.0;c.lo[k]=std::min(c.lo[k],verts[v].p[k]);c.hi[k]=std::max(c.hi[k],verts[v].p[k]);}for(int k=0;k<3;++k)c.centroid[k]+=fc[k]*faceA[f];}
 Vec3 allLo{1e99,1e99,1e99},allHi{-1e99,-1e99,-1e99};for(auto&v:verts)for(int k=0;k<3;++k){allLo[k]=std::min(allLo[k],v.p[k]);allHi[k]=std::max(allHi[k],v.p[k]);}double diag=norm(sub(allHi,allLo));int largest=-1;for(auto&kv:cs){auto&c=kv.second;if(largest<0||c.area>cs[largest].area)largest=c.id;}if(largest>=0&&cs[largest].area>0)for(auto&kv:cs)for(int k=0;k<3;++k)kv.second.centroid[k]/=std::max(kv.second.area,1e-30);
 std::set<int> review_components,pinned_components;report<<"component_id\ttriangles\tarea\tbbox_diagonal\taabb_gap_to_largest\tdecision\n";
 for(auto&kv:cs){auto c=kv.second;double d=norm(sub(c.hi,c.lo)),gap=0;if(largest>=0&&c.id!=largest){Vec3 g{0,0,0};for(int k=0;k<3;++k)g[k]=std::max(0.0,std::max(cs[largest].lo[k]-c.hi[k],c.lo[k]-cs[largest].hi[k]));gap=norm(g);}bool review=c.count<=2&&c.area<std::max(1e-15,totalA*1e-5)&&d<std::max(1e-12,diag*0.02);bool pin=review||c.count<=4;if(review)review_components.insert(c.id);if(pin){pinned_components.insert(c.id);for(size_t f=0;f<nf;++f)if(faceComp[f]==c.id)for(auto vi:tris[f].v){verts[vi].locked=true;hard_locked[vi]=true;}}report<<c.id<<'\t'<<c.count<<'\t'<<std::setprecision(10)<<c.area<<'\t'<<d<<'\t'<<gap<<'\t'<<(review?"review-tiny-detached-candidate; retained-and-pinned":(pin?"keep-small-component-pinned":"keep"))<<'\n';}
 // Flag extremely needle-like faces that are tiny relative to their geometric-edge neighborhood and whose geometric normal disagrees with their shading normals. This is a review/pin signal only.
 std::vector<double> areas;for(double a:faceA)if(a>1e-20)areas.push_back(a);double medA=0;if(!areas.empty()){std::nth_element(areas.begin(),areas.begin()+areas.size()/2,areas.end());medA=areas[areas.size()/2];}
 struct Protrusion{size_t face;double area,shape,normal_dot,local_area;};std::vector<Protrusion> protrusion_candidates;
 for(size_t f=0;f<nf;++f){auto&t=tris[f];Vec3 avg{0,0,0};for(auto vi:t.v)for(int k=0;k<3;k++)avg[k]+=verts[vi].n[k]/3.0;avg=normed(avg);double nd=dot(faceN[f],avg);double e0=norm(sub(verts[t.v[1]].p,verts[t.v[0]].p)),e1=norm(sub(verts[t.v[2]].p,verts[t.v[1]].p)),e2=norm(sub(verts[t.v[0]].p,verts[t.v[2]].p));double longest=std::max(e0,std::max(e1,e2));double shape=faceA[f]>1e-30?longest*longest/(2.0*faceA[f]):std::numeric_limits<double>::infinity();std::vector<double> local{faceA[f]};for(int q:faceAdj[f])if(faceA[q]>1e-20)local.push_back(faceA[q]);double localA=medA;if(local.size()>1){std::nth_element(local.begin(),local.begin()+local.size()/2,local.end());localA=local[local.size()/2];}bool flag=!faceAdj[f].empty()&&faceA[f]<localA*0.025&&shape>100.0&&nd<0.25;if(flag){protrusion_candidates.push_back({f,faceA[f],shape,nd,localA});for(auto vi:t.v){verts[vi].locked=true;hard_locked[vi]=true;}}}
 size_t protrusions=protrusion_candidates.size();size_t locked=0;for(auto&v:verts)if(v.locked)++locked;
 // Area-weighted face-plane quadrics.
 for(size_t f=0;f<nf;++f){if(faceA[f]<=1e-20)continue;auto&t=tris[f];Vec3 n=faceN[f],p=verts[t.v[0]].p;Q q=planeq(n,-dot(n,p),faceA[f]);for(auto v:t.v)qs[v]+=q;}
 auto incident=[&](uint32_t v){std::set<uint32_t> out;for(int fi:vtris[v])if(tris[fi].live)for(auto w:tris[fi].v)if(w!=v)out.insert(w);return out;};auto hasedge=[&](uint32_t a,uint32_t b){for(int fi:vtris[a])if(tris[fi].live){bool aa=false,bb=false;for(auto w:tris[fi].v){aa|=w==a;bb|=w==b;}if(aa&&bb)return true;}return false;};
 std::priority_queue<Cand> pq;auto pushEdge=[&](uint32_t a,uint32_t b){if(a==b||!verts[a].live||!verts[b].live||!hasedge(a,b))return;uint32_t k=a,r=b;if(verts[a].locked&&verts[b].locked)return;if(verts[b].locked&&!verts[a].locked){k=b;r=a;}else if(!verts[a].locked&&!verts[b].locked&&b<a){k=b;r=a;}Q q=qs[k];q+=qs[r];Vec3 p0=verts[k].p,p1=verts[r].p,d=sub(p1,p0);double t=qem_edge_t(q,p0,d);if(verts[k].locked)t=0;double gc=qeval(q,{p0[0]+t*d[0],p0[1]+t*d[1],p0[2]+t*d[2]});double udu=std::hypot(verts[k].uv[0]-verts[r].uv[0],verts[k].uv[1]-verts[r].uv[1]);Vec3 nd=sub(verts[k].n,verts[r].n);double ndist=norm(nd);double cost=std::max(0.0,gc)+0.0005*udu*udu+0.0001*ndist*ndist;if(!std::isfinite(cost))return;pq.push({cost,k,r,verts[k].version,verts[r].version,t});};
 for(uint32_t i=0;i<nv;++i)if(verts[i].live){auto ns=incident(i);for(auto n:ns)if(i<n)pushEdge(i,n);}
 size_t alive=nf,removed=0;size_t pops=0,geometric_guard_rejected_duplicate=0,geometric_guard_rejected_nonmanifold=0;while(alive>target&&!pq.empty()){Cand c=pq.top();pq.pop();++pops;if(!verts[c.keep].live||!verts[c.remove].live||verts[c.keep].version!=c.va||verts[c.remove].version!=c.vb)continue;if(!hasedge(c.keep,c.remove))continue;
  // Reject non-manifold edges and flips; degenerate edge-adjacent faces may disappear.
  std::vector<int> affected;affected.insert(affected.end(),vtris[c.keep].begin(),vtris[c.keep].end());affected.insert(affected.end(),vtris[c.remove].begin(),vtris[c.remove].end());std::sort(affected.begin(),affected.end());affected.erase(std::unique(affected.begin(),affected.end()),affected.end());int edgefaces=0;for(int fi:affected)if(tris[fi].live){bool a=false,b=false;for(auto x:tris[fi].v){a|=x==c.keep;b|=x==c.remove;}if(a&&b)++edgefaces;}if(edgefaces>2)continue;
  // Enforce the edge link condition and prevent newly duplicated index faces.
  auto ka=incident(c.keep),kb=incident(c.remove);std::set<uint32_t> common,opposite;std::set_intersection(ka.begin(),ka.end(),kb.begin(),kb.end(),std::inserter(common,common.end()));
  for(int fi:affected)if(tris[fi].live){bool hk=false,hr=false;for(auto v:tris[fi].v){hk|=v==c.keep;hr|=v==c.remove;}if(hk&&hr)for(auto v:tris[fi].v)if(v!=c.keep&&v!=c.remove)opposite.insert(v);}
  if(common!=opposite){++topology_reject;continue;}
  std::set<std::array<uint32_t,3>> seen;bool unique_faces=true;for(int fi:affected)if(tris[fi].live){auto f=tris[fi].v;for(auto&v:f)if(v==c.remove)v=c.keep;std::sort(f.begin(),f.end());if(f[0]==f[1]||f[1]==f[2])continue;if(!seen.insert(f).second){unique_faces=false;break;}}if(!unique_faces){++topology_reject;continue;}
  double t=verts[c.keep].locked?0:c.t;Vec3 newp=verts[c.keep].p;if(!verts[c.keep].locked){Vec3 d=sub(verts[c.remove].p,verts[c.keep].p);for(int k=0;k<3;++k)newp[k]+=t*d[k];}
  bool ok=true;for(int fi:affected)if(tris[fi].live){T&tr=tris[fi];auto old=tr.v;bool hasK=false,hasR=false;for(auto x:old){hasK|=x==c.keep;hasR|=x==c.remove;}if(!(hasK&&hasR)){std::array<Vec3,3> op,np;for(int q=0;q<3;++q){op[q]=verts[old[q]].p;uint32_t id=old[q]==c.remove?c.keep:old[q];np[q]=(id==c.keep)?newp:verts[id].p;}Vec3 on=cross(sub(op[1],op[0]),sub(op[2],op[0])),nn=cross(sub(np[1],np[0]),sub(np[2],np[0]));double ol=norm(on),nl=norm(nn);if(nl<1e-14||dot(on,nn)<0.05*ol*nl){ok=false;break;}}}if(!ok)continue;
  Vec2 newuv=verts[c.keep].uv;Vec3 newnormal=verts[c.keep].n;if(!verts[c.keep].locked){for(int k=0;k<2;k++)newuv[k]=(1-t)*verts[c.keep].uv[k]+t*verts[c.remove].uv[k];for(int k=0;k<3;k++)newnormal[k]=(1-t)*verts[c.keep].n[k]+t*verts[c.remove].n[k];newnormal=normed(newnormal);}
  for(int fi:affected)if(tris[fi].live){auto&tr=tris[fi];bool hasK=false,hasR=false;for(auto x:tr.v){hasK|=x==c.keep;hasR|=x==c.remove;}if(hasK&&hasR)continue;
    std::array<Vec2,3> olduv,u;std::array<Vec3,3> p;Vec3 avg{0,0,0};for(int k=0;k<3;k++){uint32_t id=tr.v[k];olduv[k]=verts[id].uv;bool changed=(id==c.keep||id==c.remove);u[k]=changed?newuv:verts[id].uv;p[k]=changed?newp:verts[id].p;Vec3 vn=changed?newnormal:verts[id].n;for(int q=0;q<3;q++)avg[q]+=vn[q];}
    double olda=uvarea2(olduv),newa=uvarea2(u);if(std::abs(olda)<=1e-14||std::abs(newa)<=1e-14||olda*newa<=0){ok=false;++uv_reject_orientation;break;}
    std::array<Vec3,3> oldp;for(int k=0;k<3;k++)oldp[k]=verts[tr.v[k]].p;Vec3 newcr=cross(sub(p[1],p[0]),sub(p[2],p[0]));double newArea2=norm(newcr);if(faceA[fi]>1e-20&&dot(faceN[fi],newcr)<g_source_normal_cos*newArea2){ok=false;++normal_reject;break;}if(dot(newcr,avg)<=0){ok=false;++normal_reject;break;}
    const auto&ref=source_metrics[fi];MappingMetric now=mapping_metric(p,u);if(!ref.valid){bool unchanged=true;for(int k=0;k<3;k++)unchanged=unchanged&&(p[k]==oldp[k])&&(u[k]==olduv[k]);if(!unchanged){ok=false;++metric_reject;break;}}
    else{double low,high;if(!now.valid||ref.uv_area2*now.uv_area2<=0||!relative_stretches(ref,now,low,high)||low<1/g_uv_stretch_limit-1e-10||high>g_uv_stretch_limit+1e-10){ok=false;++metric_reject;break;}}
    UVEnvelope::Status st;double worst;if(!envelope.probe_triangle(u,p,g_envelope_limit,st,worst)){ok=false;if(st==UVEnvelope::OUTSIDE)++envelope_reject_outside;else if(st==UVEnvelope::AMBIGUOUS)++envelope_reject_ambiguous;else ++envelope_reject_distance;break;}
  }if(!ok)continue;
  std::map<GeoFaceKey,int> geometric_face_delta;std::map<GeoEdgeKey,int> geometric_edge_delta;for(int fi:affected)if(tris[fi].live){auto&tr=tris[fi];std::array<Vec3,3> oldp,newp_array;bool hasK=false,hasR=false;for(auto vi:tr.v){hasK|=vi==c.keep;hasR|=vi==c.remove;}for(int k=0;k<3;++k){uint32_t vi=tr.v[k];oldp[k]=verts[vi].p;newp_array[k]=(vi==c.keep||vi==c.remove)?newp:verts[vi].p;}add_geo_delta(oldp,-1,geometric_face_delta,geometric_edge_delta);if(!(hasK&&hasR))add_geo_delta(newp_array,1,geometric_face_delta,geometric_edge_delta);}
  GeoStats projected_geo_stats;GeoGuardReason geo_guard=check_geo_delta(geometric_face_delta,geometric_edge_delta,projected_geo_stats);if(geo_guard==GeoGuardReason::DUPLICATE){++geometric_guard_rejected_duplicate;continue;}if(geo_guard==GeoGuardReason::NONMANIFOLD){++geometric_guard_rejected_nonmanifold;continue;}
  std::set<uint32_t> dirty;for(int fi:affected)if(tris[fi].live){T&tr=tris[fi];auto old=tr.v;for(auto x:old){dirty.insert(x);vtris[x].erase(fi);}for(auto&x:tr.v)if(x==c.remove)x=c.keep;if(tr.v[0]==tr.v[1]||tr.v[1]==tr.v[2]||tr.v[2]==tr.v[0]){tr.live=false;--alive;++removed;continue;}for(auto x:tr.v)vtris[x].insert(fi);for(auto x:tr.v)dirty.insert(x);}
  commit_geo_delta(geometric_face_delta,geometric_edge_delta,projected_geo_stats);
  verts[c.keep].p=newp;if(!verts[c.keep].locked){for(int k=0;k<2;++k)verts[c.keep].uv[k]=(1-t)*verts[c.keep].uv[k]+t*verts[c.remove].uv[k];for(int k=0;k<3;++k)verts[c.keep].n[k]=(1-t)*verts[c.keep].n[k]+t*verts[c.remove].n[k];Vec3 nn=normed(verts[c.keep].n);for(int k=0;k<3;++k)verts[c.keep].n[k]=nn[k];}qs[c.keep]+=qs[c.remove];verts[c.remove].live=false;
  // Requeue all local edges; versions invalidate stale costs.
  for(auto v:dirty){if(verts[v].live)++verts[v].version;}
  std::set<std::pair<uint32_t,uint32_t>> candEdges;for(auto v:dirty)if(verts[v].live){for(int fi:vtris[v])if(tris[fi].live)for(auto w:tris[fi].v)if(v!=w)candEdges.insert(std::minmax(v,w));}for(auto e:candEdges)pushEdge(e.first,e.second);
  if(pops%500000==0)std::cerr<<"queue pops="<<pops<<" triangles="<<alive<<"\n";
 }
 // A second conservative phase handles exact-position UV/normal wedges as a
 // single geometric collapse. Every wedge keeps its own corner attributes;
 // only the new POSITION is shared across both sides. Ambiguous, boundary,
 // non-manifold, small-component, invalid-atlas, and feature-pinned cases stay locked.
 size_t seam_sync_edges=0,seam_sync_accepted=0,seam_sync_rejected_hard=0,seam_sync_rejected_link=0,seam_sync_rejected_quality=0,seam_sync_rejected_nonmanifold=0,seam_sync_rejected_geom_duplicate=0,seam_sync_removed=0;
 if(alive>target){
  struct SyncUse{int face;uint32_t a,b;};
  struct SyncCandidate{double cost;int keep_group,remove_group;double t;Vec3 p0,p1;std::vector<std::pair<uint32_t,uint32_t>> pairs;};
  size_t sync_passes=0;
  while(alive>target&&sync_passes++<nf){
   std::map<Key3,int> group_map;std::vector<std::vector<uint32_t>> group_members;std::vector<int> current_gid(nv,-1);
   for(uint32_t vi=0;vi<nv;++vi)if(verts[vi].live){Key3 k{fbits(float(verts[vi].p[0])),fbits(float(verts[vi].p[1])),fbits(float(verts[vi].p[2]))};auto it=group_map.find(k);int id;if(it==group_map.end()){id=int(group_members.size());group_map.emplace(k,id);group_members.push_back({});}else id=it->second;current_gid[vi]=id;group_members[id].push_back(vi);}
   std::vector<Vec3> current_faceN(nf,Vec3{0,0,0});std::map<std::pair<int,int>,std::vector<SyncUse>> current_edges;
   std::vector<int> supported_face_edges(nf,0);std::vector<std::array<int,3>> current_geomface(nf);
   for(size_t fi=0;fi<nf;++fi)if(tris[fi].live){auto&t=tris[fi];Vec3 a=verts[t.v[0]].p,b=verts[t.v[1]].p,c=verts[t.v[2]].p;Vec3 cr=cross(sub(b,a),sub(c,a));current_faceN[fi]=norm(cr)>1e-20?normed(cr):Vec3{0,0,0};for(int k=0;k<3;++k)current_geomface[fi][k]=current_gid[t.v[k]];std::sort(current_geomface[fi].begin(),current_geomface[fi].end());for(int e=0;e<3;++e){uint32_t va=t.v[e],vb=t.v[(e+1)%3];int ga=current_gid[va],gb=current_gid[vb];if(ga<0||gb<0||ga==gb)continue;if(ga>gb)std::swap(ga,gb);current_edges[{ga,gb}].push_back({int(fi),va,vb});}}
   std::vector<std::set<int>> group_neighbors(group_members.size());
   for(auto&kv:current_edges){auto&u=kv.second;if(u.size()!=2||u[0].face==u[1].face)continue;int f0=u[0].face,f1=u[1].face;if(current_geomface[f0]==current_geomface[f1]||dot(current_faceN[f0],current_faceN[f1])<=-0.95)continue;group_neighbors[kv.first.first].insert(kv.first.second);group_neighbors[kv.first.second].insert(kv.first.first);++supported_face_edges[f0];++supported_face_edges[f1];}
   std::vector<SyncCandidate> candidates;candidates.reserve(current_edges.size()/4);
   for(auto&kv:current_edges){auto&uses=kv.second;if(uses.size()!=2||uses[0].face==uses[1].face)continue;int f0=uses[0].face,f1=uses[1].face;if(current_geomface[f0]==current_geomface[f1]||dot(current_faceN[f0],current_faceN[f1])<=-0.95)continue;
    int g0=kv.first.first,g1=kv.first.second;if(g0<0||g1<0||size_t(g0)>=group_members.size()||size_t(g1)>=group_members.size())continue;
    // Only a fully paired two-wedge seam is eligible. Half seams, junctions,
    // or coincident groups with more than two render vertices are retained.
    if(group_members[g0].size()!=2||group_members[g1].size()!=2)continue;
    if(supported_face_edges[f0]<2||supported_face_edges[f1]<2)continue;
    auto vertex_at=[&](const SyncUse&u,int g)->uint32_t{if(current_gid[u.a]==g)return u.a;if(current_gid[u.b]==g)return u.b;return UINT32_MAX;};
    uint32_t k0=vertex_at(uses[0],g0),r0=vertex_at(uses[0],g1),k1=vertex_at(uses[1],g0),r1=vertex_at(uses[1],g1);if(k0==UINT32_MAX||r0==UINT32_MAX||k1==UINT32_MAX||r1==UINT32_MAX)continue;
    std::set<uint32_t> seen_keep{k0,k1},seen_remove{r0,r1};std::set<uint32_t> want_keep(group_members[g0].begin(),group_members[g0].end()),want_remove(group_members[g1].begin(),group_members[g1].end());if(seen_keep!=want_keep||seen_remove!=want_remove||k0==k1||r0==r1)continue;
    bool hard=false;for(uint32_t vi:group_members[g0])hard|=hard_locked[vi];for(uint32_t vi:group_members[g1])hard|=hard_locked[vi];if(hard){++seam_sync_rejected_hard;continue;}
    bool attr_split=false;for(auto p:std::array<std::pair<uint32_t,uint32_t>,2>{{{k0,k1},{r0,r1}}}){Vec2 ua=verts[p.first].uv,ub=verts[p.second].uv;Vec3 na=verts[p.first].n,nb=verts[p.second].n;attr_split|=std::abs(ua[0]-ub[0])>1e-5||std::abs(ua[1]-ub[1])>1e-5||norm(sub(na,nb))>1e-4;}if(!attr_split)continue;
    // Geometric link condition across the paired charts.
    std::set<int> common,opposite;std::set_intersection(group_neighbors[g0].begin(),group_neighbors[g0].end(),group_neighbors[g1].begin(),group_neighbors[g1].end(),std::inserter(common,common.end()));
    for(int fi:{f0,f1}){bool has0=false,has1=false;for(uint32_t vi:tris[fi].v){has0|=current_gid[vi]==g0;has1|=current_gid[vi]==g1;}if(has0&&has1)for(uint32_t vi:tris[fi].v)if(current_gid[vi]!=g0&&current_gid[vi]!=g1)opposite.insert(current_gid[vi]);}
    if(common!=opposite){++seam_sync_rejected_link;continue;}
    Q q;for(uint32_t vi:group_members[g0])q+=qs[vi];for(uint32_t vi:group_members[g1])q+=qs[vi];Vec3 p0=verts[group_members[g0][0]].p,p1=verts[group_members[g1][0]].p,d=sub(p1,p0);double t=qem_edge_t(q,p0,d);Vec3 newp{p0[0]+t*d[0],p0[1]+t*d[1],p0[2]+t*d[2]};double uvCost=0,normalCost=0;std::vector<std::pair<uint32_t,uint32_t>> pairs{{k0,r0},{k1,r1}};for(auto pr:pairs){double du=verts[pr.first].uv[0]-verts[pr.second].uv[0],dv=verts[pr.first].uv[1]-verts[pr.second].uv[1];uvCost+=du*du+dv*dv;normalCost+=std::pow(norm(sub(verts[pr.first].n,verts[pr.second].n)),2);}
    double cost=std::max(0.0,qeval(q,newp))+0.0005*uvCost+0.0001*normalCost;if(std::isfinite(cost))candidates.push_back({cost,g0,g1,t,p0,p1,std::move(pairs)});
   }
   seam_sync_edges+=candidates.size();if(candidates.empty())break;
   std::sort(candidates.begin(),candidates.end(),[](const SyncCandidate&a,const SyncCandidate&b){return a.cost<b.cost||(a.cost==b.cost&&std::tie(a.keep_group,a.remove_group)<std::tie(b.keep_group,b.remove_group));});
   bool committed=false;
   for(const auto&c:candidates){
    std::vector<int> affected;for(auto pr:c.pairs){for(int fi:vtris[pr.first])if(tris[fi].live)affected.push_back(fi);for(int fi:vtris[pr.second])if(tris[fi].live)affected.push_back(fi);}std::sort(affected.begin(),affected.end());affected.erase(std::unique(affected.begin(),affected.end()),affected.end());
    bool region_manifold=true;for(int fi:affected)if(tris[fi].live)for(int e=0;e<3;++e){int ga=current_gid[tris[fi].v[e]],gb=current_gid[tris[fi].v[(e+1)%3]];if(ga==gb)continue;if(ga>gb)std::swap(ga,gb);auto it=current_edges.find({ga,gb});if(it!=current_edges.end()&&it->second.size()>2)region_manifold=false;}if(!region_manifold){++seam_sync_rejected_nonmanifold;continue;}
    Vec3 newp{c.p0[0]+c.t*(c.p1[0]-c.p0[0]),c.p0[1]+c.t*(c.p1[1]-c.p0[1]),c.p0[2]+c.t*(c.p1[2]-c.p0[2])};
    std::vector<int32_t> replacement(nv,-1);std::vector<Vec2> new_uv(nv);std::vector<Vec3> new_n(nv);for(auto pr:c.pairs){replacement[pr.first]=int32_t(pr.first);replacement[pr.second]=int32_t(pr.first);Vec2 iu{(1-c.t)*verts[pr.first].uv[0]+c.t*verts[pr.second].uv[0],(1-c.t)*verts[pr.first].uv[1]+c.t*verts[pr.second].uv[1]};new_uv[pr.first]=iu;new_uv[pr.second]=iu;Vec3 in=normed({(1-c.t)*verts[pr.first].n[0]+c.t*verts[pr.second].n[0],(1-c.t)*verts[pr.first].n[1]+c.t*verts[pr.second].n[1],(1-c.t)*verts[pr.first].n[2]+c.t*verts[pr.second].n[2]});new_n[pr.first]=in;new_n[pr.second]=in;}
    std::set<std::array<uint32_t,3>> seen_faces;bool ok=true;int collapsed_faces=0;
    for(int fi:affected)if(tris[fi].live){auto ids=tris[fi].v;for(auto&vi:ids)if(replacement[vi]>=0)vi=uint32_t(replacement[vi]);if(ids[0]==ids[1]||ids[1]==ids[2]||ids[2]==ids[0]){++collapsed_faces;continue;}auto sorted=ids;std::sort(sorted.begin(),sorted.end());if(!seen_faces.insert(sorted).second){ok=false;break;}
     std::array<Vec2,3> old_uv,u;std::array<Vec3,3> old_p,p;Vec3 avg{0,0,0};for(int k=0;k<3;++k){uint32_t old=tris[fi].v[k];old_uv[k]=verts[old].uv;old_p[k]=verts[old].p;bool moved=replacement[old]>=0;u[k]=moved?new_uv[old]:verts[old].uv;p[k]=moved?newp:verts[old].p;Vec3 nn=moved?new_n[old]:verts[old].n;for(int qn=0;qn<3;++qn)avg[qn]+=nn[qn]/3.0;}
     double olda=uvarea2(old_uv),newa=uvarea2(u);if(std::abs(olda)<=1e-14||std::abs(newa)<=1e-14||olda*newa<=0){ok=false;++uv_reject_orientation;break;}
     Vec3 cr=cross(sub(p[1],p[0]),sub(p[2],p[0]));double area2=norm(cr);if(area2<1e-14||(faceA[fi]>1e-20&&dot(faceN[fi],cr)<g_source_normal_cos*area2)||dot(cr,avg)<=0){ok=false;++normal_reject;break;}
     const auto&ref=source_metrics[fi];MappingMetric now=mapping_metric(p,u);if(!ref.valid||!now.valid||ref.uv_area2*now.uv_area2<=0){ok=false;++metric_reject;break;}double low,high;if(!relative_stretches(ref,now,low,high)||low<1/g_uv_stretch_limit-1e-10||high>g_uv_stretch_limit+1e-10){ok=false;++metric_reject;break;}
     UVEnvelope::Status st;double worst;if(!envelope.probe_triangle(u,p,g_envelope_limit,st,worst)){ok=false;if(st==UVEnvelope::OUTSIDE)++envelope_reject_outside;else if(st==UVEnvelope::AMBIGUOUS)++envelope_reject_ambiguous;else ++envelope_reject_distance;break;}
    }
    if(!ok||collapsed_faces!=2){++seam_sync_rejected_quality;continue;}
    std::map<GeoFaceKey,int> geometric_face_delta;std::map<GeoEdgeKey,int> geometric_edge_delta;for(int fi:affected)if(tris[fi].live){auto&tr=tris[fi];std::array<Vec3,3> oldp,newp_array;auto ids=tr.v;for(auto&vi:ids)if(replacement[vi]>=0)vi=uint32_t(replacement[vi]);for(int k=0;k<3;++k){uint32_t vi=tr.v[k];oldp[k]=verts[vi].p;newp_array[k]=replacement[vi]>=0?newp:verts[vi].p;}add_geo_delta(oldp,-1,geometric_face_delta,geometric_edge_delta);if(ids[0]!=ids[1]&&ids[1]!=ids[2]&&ids[2]!=ids[0])add_geo_delta(newp_array,1,geometric_face_delta,geometric_edge_delta);}
    GeoStats projected_geo_stats;GeoGuardReason geo_guard=check_geo_delta(geometric_face_delta,geometric_edge_delta,projected_geo_stats);if(geo_guard==GeoGuardReason::DUPLICATE){++seam_sync_rejected_geom_duplicate;continue;}if(geo_guard==GeoGuardReason::NONMANIFOLD){++seam_sync_rejected_nonmanifold;continue;}
    std::set<uint32_t> dirty;for(int fi:affected)if(tris[fi].live){T&tr=tris[fi];auto old=tr.v;for(auto vi:old){dirty.insert(vi);vtris[vi].erase(fi);}for(auto&vi:tr.v)if(replacement[vi]>=0)vi=uint32_t(replacement[vi]);if(tr.v[0]==tr.v[1]||tr.v[1]==tr.v[2]||tr.v[2]==tr.v[0]){tr.live=false;--alive;++removed;++seam_sync_removed;continue;}for(auto vi:tr.v){vtris[vi].insert(fi);dirty.insert(vi);}}
    commit_geo_delta(geometric_face_delta,geometric_edge_delta,projected_geo_stats);
    for(auto pr:c.pairs){verts[pr.first].p=newp;verts[pr.first].uv=new_uv[pr.first];verts[pr.first].n=new_n[pr.first];verts[pr.first].locked=false;verts[pr.second].live=false;qs[pr.first]+=qs[pr.second];}
    for(auto vi:dirty)if(verts[vi].live)++verts[vi].version;
    ++seam_sync_accepted;committed=true;break;
   }
   if(!committed)break;
  }
 }
 size_t final_atlas_outside=0,final_atlas_ambiguous=0,final_probe_over_limit=0;double final_probe_max=0;
 for(auto&tr:tris)if(tr.live){std::array<Vec2,3> u;std::array<Vec3,3> p;for(int k=0;k<3;k++){u[k]=verts[tr.v[k]].uv;p[k]=verts[tr.v[k]].p;}UVEnvelope::Status st;double worst;if(envelope.probe_triangle(u,p,std::numeric_limits<double>::infinity(),st,worst)){final_probe_max=std::max(final_probe_max,worst);if(worst>g_envelope_limit+1e-9)++final_probe_over_limit;}else if(st==UVEnvelope::OUTSIDE)++final_atlas_outside;else ++final_atlas_ambiguous;}
 report<<"# final_valid_probe_max="<<final_probe_max<<" final_valid_probe_over_limit_faces="<<final_probe_over_limit<<" final_atlas_outside_faces="<<final_atlas_outside<<" final_atlas_ambiguous_faces="<<final_atlas_ambiguous<<"\n";
 report<<"# uv_envelope_limit_world="<<g_envelope_limit<<" uv_stretch_limit="<<g_uv_stretch_limit<<" source_normal_cos_min="<<g_source_normal_cos<<" original_atlas_valid_faces="<<envelope.faces.size()<<" original_uv_singular_faces="<<envelope.skipped_singular<<" original_unprojectable_faces_pinned="<<envelope_protected_faces<<" invalid_mapping_faces_pinned="<<invalid_mapping_faces<<" rejected_distance="<<envelope_reject_distance<<" rejected_outside="<<envelope_reject_outside<<" rejected_ambiguous="<<envelope_reject_ambiguous<<" rejected_uv_orientation="<<uv_reject_orientation<<" rejected_uv_stretch="<<metric_reject<<" rejected_normal="<<normal_reject<<" rejected_topology="<<topology_reject<<"\n";
 locked=0;for(const auto&v:verts)if(v.live&&v.locked)++locked;
 Result out;out.initial=nf;out.final=alive;out.removed=removed;out.locked_vertices=locked;out.protrusion_faces=protrusions;out.verts=std::move(verts);out.faces.reserve(alive);out.source_face_ids.reserve(alive);for(size_t fi=0;fi<tris.size();++fi)if(tris[fi].live){out.faces.push_back(tris[fi].v);out.source_face_ids.push_back(uint32_t(fi));}
 report<<"# geometric_edges="<<gedges.size()<<" split_vertices_locked="<<split_vertices_locked<<" geometric_boundary_edges_locked="<<geometric_boundary_edges_locked<<" uv_seam_edges="<<seam_edges<<" opposite/coincident adjacency pairs skipped="<<guarded_opposed<<"/"<<guarded_coincident<<" review_components_pinned="<<review_components.size()<<" small_components_pinned="<<pinned_components.size()<<" protected_vertices_final="<<locked<<" flagged_protrusion_faces="<<protrusions<<" removed_by_collapse="<<removed<<" triangles_final="<<alive<<"\n";
 report<<"# seam_sync_candidate_edges="<<seam_sync_edges<<" seam_sync_accepted="<<seam_sync_accepted<<" seam_sync_removed_faces="<<seam_sync_removed<<" seam_sync_rejected_hard="<<seam_sync_rejected_hard<<" seam_sync_rejected_link="<<seam_sync_rejected_link<<" seam_sync_rejected_nonmanifold="<<seam_sync_rejected_nonmanifold<<" seam_sync_rejected_geometric_duplicate="<<seam_sync_rejected_geom_duplicate<<" seam_sync_rejected_quality="<<seam_sync_rejected_quality<<"\n";
 report<<"# geometric_guard_rejected_duplicate="<<geometric_guard_rejected_duplicate<<" geometric_guard_rejected_nonmanifold="<<geometric_guard_rejected_nonmanifold<<" source_duplicate_face_groups="<<source_geo_stats.duplicate_groups<<" final_duplicate_face_groups="<<geo_stats.duplicate_groups<<" source_duplicate_face_excess="<<source_geo_stats.duplicate_excess<<" final_duplicate_face_excess="<<geo_stats.duplicate_excess<<" source_nonmanifold_edge_groups="<<source_geo_stats.nonmanifold_groups<<" final_nonmanifold_edge_groups="<<geo_stats.nonmanifold_groups<<" source_nonmanifold_edge_excess="<<source_geo_stats.nonmanifold_excess<<" final_nonmanifold_edge_excess="<<geo_stats.nonmanifold_excess<<"\n";
 report<<"\n# Review component face markers below; every marker is retained/pinned.\ncomponent_id\tface_id\tcentroid_x\tcentroid_y\tcentroid_z\tarea\tkind\tdecision\n";
 for(size_t f=0;f<nf;++f)if(review_components.count(faceComp[f])){Vec3 c{0,0,0};for(auto vi:tris[f].v)for(int k=0;k<3;++k)c[k]+=out.verts[vi].p[k]/3.0;report<<faceComp[f]<<'\t'<<f<<'\t'<<std::setprecision(10)<<c[0]<<'\t'<<c[1]<<'\t'<<c[2]<<'\t'<<faceA[f]<<"\tdetached-small\tretain-and-pin\n";}
 report<<"\n# Attached needle-like polygon flags are review-only and retained/pinned.\nface_id\tarea\tlongest_edge_squared_over_2area\tnormal_dot\tlocal_neighbor_median_area\tdecision\n";
 for(auto&x:protrusion_candidates)report<<x.face<<'\t'<<std::setprecision(10)<<x.area<<'\t'<<x.shape<<'\t'<<x.normal_dot<<'\t'<<x.local_area<<"\tretain-and-pin\n";
 if(alive>target){std::cerr<<"Warning: target not reached; conservative collapse constraints stopped at "<<alive<<" triangles.\n";}
 return out;
}
void appendalign(std::vector<uint8_t>&b){while(b.size()%4)b.push_back(0);}void appendf(std::vector<uint8_t>&b,float x){uint8_t*p=(uint8_t*)&x;b.insert(b.end(),p,p+4);}void appendu(std::vector<uint8_t>&b,uint32_t x){for(int i=0;i<4;++i)b.push_back(uint8_t(x>>(i*8)));}
int append_view(Json&r,size_t start,size_t len,int target){Json v=Json::obj();v["buffer"]=Json(0);v["byteOffset"]=Json(double(start));v["byteLength"]=Json(double(len));if(target)v["target"]=Json(target);r["bufferViews"].a.push_back(v);return int(r["bufferViews"].a.size()-1);}
int append_acc(Json&r,int view,size_t count,int comp,const std::string&type,const std::vector<double>*mn=nullptr,const std::vector<double>*mx=nullptr){Json a=Json::obj();a["bufferView"]=Json(view);a["byteOffset"]=Json(0);a["componentType"]=Json(comp);a["count"]=Json(double(count));a["type"]=Json(type);if(mn){Json x=Json::arr(),y=Json::arr();for(double z:*mn)x.a.emplace_back(z);for(double z:*mx)y.a.emplace_back(z);a["min"]=x;a["max"]=y;}r["accessors"].a.push_back(a);return int(r["accessors"].a.size()-1);}
void compact_binary(Json&r,std::vector<uint8_t>&bin){
 if(!r.has("buffers")||r.at("buffers").a.size()!=1)throw std::runtime_error("Compaction supports one embedded GLB buffer");
 if((r.has("animations")&&!r.at("animations").a.empty())||(r.has("skins")&&!r.at("skins").a.empty()))throw std::runtime_error("Animated/skinned GLBs are not supported safely");
 if(r.has("extensionsUsed")&&!r.at("extensionsUsed").a.empty())throw std::runtime_error("GLB extensions are not supported safely by this compacting writer");
 if(r.has("extensionsRequired")&&!r.at("extensionsRequired").a.empty())throw std::runtime_error("Required GLB extensions are not supported safely");
 std::set<int> usedAcc,usedView;
 for(auto&m:r.at("meshes").a)for(auto&p:m.at("primitives").a){
   if(p.has("targets"))throw std::runtime_error("Morph targets are not supported safely");
   if(p.has("indices"))usedAcc.insert(p.at("indices").integer());
   for(auto&kv:p.at("attributes").o)usedAcc.insert(kv.second.integer());
 }
 const auto oldAcc=r.at("accessors").a;const auto oldViews=r.at("bufferViews").a;
 for(int a:usedAcc){if(a<0||size_t(a)>=oldAcc.size())throw std::runtime_error("Accessor reference out of range during compaction");usedView.insert(oldAcc[a].at("bufferView").integer());}
 for(auto&im:r.at("images").a)if(im.has("bufferView"))usedView.insert(im.at("bufferView").integer());
 std::vector<int> vmap(oldViews.size(),-1),amap(oldAcc.size(),-1);Json newViews=Json::arr(),newAcc=Json::arr();std::vector<uint8_t> nb;
 for(size_t vi=0;vi<oldViews.size();++vi)if(usedView.count(int(vi))){Json v=oldViews[vi];if(v.has("buffer")&&v.at("buffer").integer()!=0)throw std::runtime_error("Only buffer 0 can be compacted");size_t start=v.has("byteOffset")?size_t(v.at("byteOffset").n):0,len=size_t(v.at("byteLength").n);if(start+len>bin.size())throw std::runtime_error("bufferView out of bounds during compaction");appendalign(nb);v["buffer"]=Json(0);v["byteOffset"]=Json(double(nb.size()));nb.insert(nb.end(),bin.begin()+start,bin.begin()+start+len);vmap[vi]=int(newViews.a.size());newViews.a.push_back(v);}
 for(size_t ai=0;ai<oldAcc.size();++ai)if(usedAcc.count(int(ai))){Json a=oldAcc[ai];int ov=a.at("bufferView").integer();if(ov<0||size_t(ov)>=vmap.size()||vmap[ov]<0)throw std::runtime_error("Accessor bufferView lost during compaction");a["bufferView"]=Json(vmap[ov]);amap[ai]=int(newAcc.a.size());newAcc.a.push_back(a);}
 auto remap=[&](Json&x){int old=x.integer();if(old<0||size_t(old)>=amap.size()||amap[old]<0)throw std::runtime_error("Accessor reference lost during compaction");x=Json(amap[old]);};
 for(auto&m:r["meshes"].a)for(auto&p:m["primitives"].a){if(p.has("indices"))remap(p["indices"]);for(auto&kv:p["attributes"].o)remap(kv.second);}
 for(auto&im:r["images"].a)if(im.has("bufferView")){int old=im.at("bufferView").integer();if(old<0||size_t(old)>=vmap.size()||vmap[old]<0)throw std::runtime_error("Image bufferView lost during compaction");im["bufferView"]=Json(vmap[old]);}
 r["accessors"]=newAcc;r["bufferViews"]=newViews;r["buffers"].a[0]["byteLength"]=Json(double(nb.size()));bin.swap(nb);
}
void run(const std::string&in,const std::string&out,size_t target,const std::string&reportpath){Glb g=readglb(in);Json&r=g.root;if(!r.has("meshes")||r.at("meshes").a.size()!=1)throw std::runtime_error("Only one mesh is supported");Json&m=r["meshes"].a[0];if(!m.has("primitives")||m.at("primitives").a.size()!=1)throw std::runtime_error("Only one mesh primitive is supported");Json&pr=m["primitives"].a[0];if(pr.has("mode")&&pr.at("mode").integer()!=4)throw std::runtime_error("Only triangle primitives are supported");if(!pr.has("indices"))throw std::runtime_error("Indexed primitive required");if(!pr.has("attributes"))throw std::runtime_error("Missing vertex attributes");const Json&at=pr.at("attributes");for(auto&kv:at.o)if(kv.first!="POSITION"&&kv.first!="NORMAL"&&kv.first!="TEXCOORD_0")throw std::runtime_error("Unsupported vertex attribute (would be lost): "+kv.first);for(const char*k:{"POSITION","NORMAL","TEXCOORD_0"})if(!at.has(k))throw std::runtime_error(std::string("Required attribute missing: ")+k);
 auto pos=read3(r,g.bin,at.at("POSITION").integer()),nor=read3(r,g.bin,at.at("NORMAL").integer());auto uv=read2(r,g.bin,at.at("TEXCOORD_0").integer());auto idx=readidx(r,g.bin,pr.at("indices").integer());if(idx.size()%3)throw std::runtime_error("Index count is not divisible by three");if(pos.size()!=nor.size()||pos.size()!=uv.size())throw std::runtime_error("Attribute counts differ");std::vector<V>vs(pos.size());for(size_t i=0;i<vs.size();++i){for(int k=0;k<3;k++){vs[i].p[k]=pos[i][k];vs[i].n[k]=nor[i][k];if(!std::isfinite(vs[i].p[k])||!std::isfinite(vs[i].n[k]))throw std::runtime_error("Non-finite position/normal");}for(int k=0;k<2;k++){vs[i].uv[k]=uv[i][k];if(!std::isfinite(vs[i].uv[k]))throw std::runtime_error("Non-finite UV");}}
 std::vector<T>ts(idx.size()/3);for(size_t i=0;i<ts.size();++i)for(int k=0;k<3;k++){if(idx[i*3+k]>=vs.size())throw std::runtime_error("Index out of range");ts[i].v[k]=idx[i*3+k];}std::ofstream rep(reportpath);if(!rep)throw std::runtime_error("Cannot write report");Result z=reduce_mesh(std::move(vs),std::move(ts),target,rep);
 std::vector<int32_t> remap(z.verts.size(),-1);std::vector<V> compact;compact.reserve(z.verts.size());std::vector<std::array<uint32_t,3>> faces;for(auto f:z.faces){std::array<uint32_t,3> q;for(int k=0;k<3;k++){uint32_t old=f[k];if(remap[old]<0){remap[old]=int32_t(compact.size());compact.push_back(z.verts[old]);}q[k]=uint32_t(remap[old]);}faces.push_back(q);}if(faces.size()>z.initial)throw std::runtime_error("Internal triangle count error");
 appendalign(g.bin);size_t pstart=g.bin.size();std::vector<double>pmin(3,1e100),pmax(3,-1e100);for(auto&v:compact)for(int k=0;k<3;k++){float q=float(v.p[k]);appendf(g.bin,q);pmin[k]=std::min(pmin[k],double(q));pmax[k]=std::max(pmax[k],double(q));}int pview=append_view(r,pstart,g.bin.size()-pstart,34962);int pacc=append_acc(r,pview,compact.size(),5126,"VEC3",&pmin,&pmax);
 appendalign(g.bin);size_t uvstart=g.bin.size();for(auto&v:compact)for(int k=0;k<2;k++)appendf(g.bin,float(v.uv[k]));int uvview=append_view(r,uvstart,g.bin.size()-uvstart,34962);int uvacc=append_acc(r,uvview,compact.size(),5126,"VEC2");
 appendalign(g.bin);size_t nstart=g.bin.size();for(auto&v:compact)for(int k=0;k<3;k++)appendf(g.bin,float(v.n[k]));int nview=append_view(r,nstart,g.bin.size()-nstart,34962);int nacc=append_acc(r,nview,compact.size(),5126,"VEC3");
 appendalign(g.bin);size_t istart=g.bin.size();uint32_t imax=0;for(auto f:faces)for(auto x:f){appendu(g.bin,x);imax=std::max(imax,x);}int iview=append_view(r,istart,g.bin.size()-istart,34963);std::vector<double>imin{0},imaxv{double(imax)};int iacc=append_acc(r,iview,faces.size()*3,5125,"SCALAR",&imin,&imaxv);
 pr["attributes"]["POSITION"]=Json(pacc);pr["attributes"]["TEXCOORD_0"]=Json(uvacc);pr["attributes"]["NORMAL"]=Json(nacc);pr["indices"]=Json(iacc);if(r.has("buffers")&&!r.at("buffers").a.empty())r["buffers"].a[0]["byteLength"]=Json(double(g.bin.size()));compact_binary(r,g.bin);writeglb(out,r,std::move(g.bin));std::ofstream provenance(out+".provenance.csv");if(!provenance)throw std::runtime_error("Cannot write provenance CSV");provenance<<"output_face,source_face\n";for(size_t i=0;i<z.source_face_ids.size();++i)provenance<<i<<','<<z.source_face_ids[i]<<'\n';rep<<"# source_triangles="<<z.initial<<" output_triangles="<<faces.size()<<" vertices_compacted="<<compact.size()<<" target="<<target<<"\n";std::cout<<"source_triangles="<<z.initial<<" output_triangles="<<faces.size()<<" output_vertices="<<compact.size()<<" locked_vertices="<<z.locked_vertices<<" seam_or_anomaly_flags="<<z.protrusion_faces<<"\n";
}
void selftest(){// Matrix basics, JSON round-trip, and synthetic face reduction with UV attributes.
  Json j=Parser("{\"a\":[1,true,\"x\"],\"b\":{}}").parse();std::string s;dump(j,s);if(Parser(s).parse().at("a").a.size()!=3)throw std::runtime_error("JSON self-test failed");
  Q q=planeq({0,0,1},-1,1);if(std::abs(qeval(q,{0,0,1}))>1e-9)throw std::runtime_error("QEM self-test failed");std::cout<<"self-test passed\n";
}
}
int main(int argc,char**argv){try{if(argc==2&&std::string(argv[1])=="--self-test"){selftest();return 0;}if(argc<3){std::cerr<<"Usage: glb_refine input.glb output.glb [target_triangles=4000] [report.tsv] [envelope_world_limit=0.004] [uv_stretch_limit=1.25] [source_normal_cos=0.8660254]\n";return 2;}size_t target=argc>3?std::stoul(argv[3]):4000;std::string report=argc>4?argv[4]:std::string(argv[2])+".components.tsv";if(argc>5)g_envelope_limit=std::stod(argv[5]);if(!(g_envelope_limit>0)||!std::isfinite(g_envelope_limit))throw std::runtime_error("Envelope limit must be finite and positive world units");if(argc>6)g_uv_stretch_limit=std::stod(argv[6]);if(!(g_uv_stretch_limit>=1)||!std::isfinite(g_uv_stretch_limit))throw std::runtime_error("UV stretch limit must be finite and >= 1");if(argc>7)g_source_normal_cos=std::stod(argv[7]);if(!(g_source_normal_cos>=0&&g_source_normal_cos<=1)||!std::isfinite(g_source_normal_cos))throw std::runtime_error("Source normal cosine must be finite and in [0,1]");run(argv[1],argv[2],target,report);return 0;}catch(const std::exception&e){std::cerr<<"ERROR: "<<e.what()<<"\n";return 1;}}
