#ifndef EVA4_FFI_SDK_HPP
#define EVA4_FFI_SDK_HPP

#define EVA4_CCP_SDK_VERSION "0.0.1"

#include <iostream>
#include <chrono>
#include <thread>
#include <msgpack.hpp>
#include "eva4-common.h"
#include "eva4-ffi.h"

/**
 * EVA ICS SDK namespace
 */
namespace eva {

  const uint16_t ABI_VERSION = 1;

  using namespace std;

  /**
   * SDK variables and types
   */
  namespace vars {

    using namespace std;

    const auto sleepStep = chrono::milliseconds(100);

    /**
     * Timeout configuration, (initial.timeout)
     */
    struct TimeoutConfig {
      /** max allowed startup timeout */
      chrono::milliseconds startup;
      /** max allowed shutdown timeout */
      chrono::milliseconds shutdown;
      /** the default timeout */
      chrono::milliseconds _default;

      void msgpack_unpack(msgpack::object o) {
        if (o.type != msgpack::type::MAP) {
          throw msgpack::type_error();
        }
        msgpack::object_kv* p = o.via.map.ptr;
        msgpack::object_kv* const pend = o.via.map.ptr + o.via.map.size;

        for (; p != pend; ++p) {
          if (p->key.as<std::string>() == "startup") {
            startup = chrono::milliseconds((int32_t)(p->val.as<double>() * 1000));
          } else if (p->key.as<std::string>() == "shutdown") {
            shutdown = chrono::milliseconds((int32_t)(p->val.as<double>() * 1000));
          } else if (p->key.as<std::string>() == "default") {
            _default = chrono::milliseconds((int32_t)(p->val.as<double>() * 1000));
          }
        }
      }
    };

    /**
     * Core info, (initial.core)
     */
    struct CoreInfo {
      uint64_t build;
      string version;
      uint16_t eapi_version;
      string path;
      bool active;

      MSGPACK_DEFINE_MAP(build, version, eapi_version, path, active);
    };

    /**
     * The initial payload
     *
     * @tparam T Service configuration structure
     */
    template<typename T>struct Initial {
      uint16_t version;
      string system_name;
      string id;
      string data_path;
      TimeoutConfig timeout;
      CoreInfo core;
      T config;
      string user;
      bool fail_mode;
      bool fips;
      bool call_tracing;

      MSGPACK_DEFINE_MAP(version, system_name, id, data_path,
          timeout, core, config, user, fail_mode, fips, call_tracing);

      /**
       * Gets the default timeout
       *
       * @return default timeout
       */
      chrono::milliseconds getTimeout() {
        return this->timeout._default;
      }

      /**
       * Gets EVA ICS directory
       *
       * @return EVA ICS directory
       */
      string evaDir() {
        return this->core.path;
      }
    };

    /**
     * Item kinds
     */
    enum ItemKind {
      Unit,
      Sensor,
      LVar,
      LMacro,
      Any
    };

    struct errorMapStr : public map<int16_t, string>
    {
      errorMapStr()
      {
        this->operator[](-32001) = "Not found";
        this->operator[](-32002) = "Access denied";
        this->operator[](-32003) = "System error";
        this->operator[](-32004) = "Other";
        this->operator[](-32005) = "Not ready";
        this->operator[](-32006) = "Unsupported";
        this->operator[](-32007) = "Core error";
        this->operator[](-32008) = "Timeout";
        this->operator[](-32009) = "Invalid data";
        this->operator[](-32010) = "Function failed";
        this->operator[](-32011) = "Aborted";
        this->operator[](-32012) = "Resource already existS";
        this->operator[](-32013) = "Busy";
        this->operator[](-32014) = "Method not implemented";
        this->operator[](-32015) = "Token restricted";
        this->operator[](-32016) = "I/O";
        this->operator[](-32017) = "Registry";
        this->operator[](-32018) = "evaHI auth required";
        this->operator[](-32022) = "Access denied more data required";
        this->operator[](-32700) = "Parse error";
        this->operator[](-32600) = "Invalid request";
        this->operator[](-32601) = "Method not found";
        this->operator[](-32602) = "Invalid params";
        this->operator[](-32603) = "Internal RPC";
        this->operator[](-32113) = "Bus client not registered";
        this->operator[](-32114) = "Bus data";
        this->operator[](-32115) = "Bus io";
        this->operator[](-32116) = "Bus other";
        this->operator[](-32117) = "Bus not supported";
        this->operator[](-32118) = "Bus busy";
        this->operator[](-32119) = "Bus not delivered";
        this->operator[](-32120) = "Bus timeout";
        this->operator[](-32121) = "Bus access";
      };
    };

    /**
     * Error code to string mapping
     */
    errorMapStr errorMap;

    struct itemKindMap : public map<string, ItemKind>
    {
      itemKindMap()
      {
        this->operator[]("unit") =  Unit;
        this->operator[]("sensor") =  Sensor;
        this->operator[]("lvar") =  LVar;
        this->operator[]("lmacro") =  LMacro;
        this->operator[]("+") =  Any;
      };
    };

    struct itemKindMapStr : public map<ItemKind, string>
    {
      itemKindMapStr()
      {
        this->operator[](Unit) =  "unit";
        this->operator[](Sensor) =  "sensor";
        this->operator[](LVar) =  "lvar";
        this->operator[](LMacro) =  "lmacro";
        this->operator[](Any) =  "+";
      };
    };

    /**
     * Item kind string to ItemKind mapping
     */
    itemKindMap itemMapK;
    /**
     * ItemKind item kind string mapping
     */
    itemKindMapStr itemMapS;
  };

  /**
   * The generic exception class
   */
  class Exception : public exception {
    public:
      /**
       * Constructs an exception
       *
       * @param code - error code
       */
      Exception(int16_t code) {
        this->code = code;
        stringstream ss;
        auto i = vars::errorMap.find(code);
        if (i != vars::errorMap.end()) {
          ss << i->second << ' ';
        }
        ss << '(' << this->code << ')';
        this->msg = ss.str();
      }
      char * what() {
        return (char *)this->msg.c_str();
      }
      /**
       * Automatically logs the exception error to STDERR
       */
      void log() {
        cerr << this->msg << endl << flush;
      }
      /**
       * Automatically logs the exception error to STDERR
       *
       * @param context - error context
       */
      void log(string context) {
        cerr << context << ": " << this->msg << endl << flush;
      }
      int16_t code;
    private:
      string msg;
  };

  /**
   * Raw item status payload
   */
  struct RawItemStatus {
    int16_t status;

    MSGPACK_DEFINE_MAP(status);
  };

  /**
   * Raw item state payload
   *
   * @tparam T item value kind
   */
  template <typename T>
    struct RawItemState {
      int16_t status;
      T value;

      MSGPACK_DEFINE_MAP(status, value);
    };

  /**
   * A typical RPC payload for ID/OID-calls
   */
  struct CallParamsId {
    string i;

    MSGPACK_DEFINE_MAP(i);
  };

  atomic<int32_t (*)(int16_t op_code, struct EvaFFIBuffer* payload)> svc_op_fn(nullptr);

  stringstream packStrings(vector<string>& strings) {
    stringstream ss;
    for (const auto& s : strings) {
      ss << s;
      ss.put(0);
    }
    return ss;
  }

  /**
   * Call a service operation (low-level)
   *
   * @param op_code operation code
   */
  int32_t svcOp(int16_t op_code) {
    return svc_op_fn ? svc_op_fn(op_code, nullptr) : EVA_ERR_CODE_NOT_READY;
  }

  /**
   * Call a service operation (low-level)
   *
   * @param op_code operation code
   * @param ss payload string stream
   */
  int32_t svcOpSS(int16_t op_code, stringstream& ss) {
    size_t size = ss.tellp();
    char* buf = (char *)malloc(size);
    struct EvaFFIBuffer payload = EvaFFIBuffer { size, buf, size };
    memcpy(payload.data, ss.str().c_str(), size);
    int32_t res = svc_op_fn(op_code, &payload);
    free(buf);
    return res;
  }

  /**
   * Checks is the service active
   *
   * @return service active state
   */
  bool active() {
    return svcOp(EVA_FFI_SVC_OP_IS_ACTIVE) == 1;
  }

  /**
   * Converts the operation result code into exception if error
   * @param code operation result code
   *
   * @throws Exception
   */
  void c2e(int16_t code) {
    if (code < EVA_OK)  {
      throw Exception(code);
    }
  }

  /**
   * Subscribes to topics
   *
   * @param topics topics to subscribe
   *
   * @throws Exception
   */
  void subscribe(vector<string>& topics) {
    stringstream ss = packStrings(topics);
    c2e(svcOpSS(EVA_FFI_SVC_OP_SUBSCRIBE_TOPIC, ss));
  }

  /**
   * Subscribes to a topic
   *
   * @param topic topic to subscribe
   *
   * @throws Exception
   */
  void subscribe(string topic) {
    stringstream ss;
    ss << topic;
    c2e(svcOpSS(EVA_FFI_SVC_OP_SUBSCRIBE_TOPIC, ss));
  }

  /**
   * Unsubscribes from topics
   *
   * @param topics topics to unsubscribe
   *
   * @throws Exception
   */
  void unsubscribe(vector<string>& topics) {
    stringstream ss = packStrings(topics);
    c2e(svcOpSS(EVA_FFI_SVC_OP_UNSUBSCRIBE_TOPIC, ss));
  }

  /**
   * Unsubscribes from a topic
   *
   * @param topic topic to unsubscribe
   *
   * @throws Exception
   */
  void unsubscribe(string topic) {
    stringstream ss;
    ss << topic;
    c2e(svcOpSS(EVA_FFI_SVC_OP_UNSUBSCRIBE_TOPIC, ss));
  }

  /**
   * Publishes data to a topic
   *
   * @tparam T must be MessagePack-serializable
   *
   * @param topic topic to publish to
   * @param data payload to publish
   *
   * @throws Exception
   */
  template <typename T> void publish(string topic, T data) {
    stringstream ss;
    ss << topic;
    ss.put(0);
    msgpack::pack(ss, data);
    c2e(svcOpSS(EVA_FFI_SVC_OP_PUBLISH_TOPIC, ss));
  }

  /**
   * The primary OID class
   */
  class OID {
    public:
      OID() {
        this->kind = vars::ItemKind::Any;
        this->i = string();
        this->path = string();
        this->rawTopic = string();
      }
      /**
       * Constructs OID from a string
       *
       * @param s OID string
       *
       * @throws Exception
       */
      OID(string s) {
        this->parse(s, false);
      }
      /**
       * Constructs OID from a string or a path
       *
       * @param s OID string or path
       * @param fromPath true if the string is a path
       *
       * @throws Exception
       */
      OID(string s, bool fromPath) {
        this->parse(s, fromPath);
      }
      /**
       * Gets item full id (without a kind)
       *
       * @return item full id
       */
      string fullId() {
        return this->i.substr(this->pos + 1);
      }
      /**
       * Sets item status to 1 (OK) for the current OID
       *
       * @throws Exception
       */
      void markOk() {
        publish(this->rawTopic, RawItemStatus{ EVA_ITEM_STATUS_OK });
      }
      /**
       * Sets item status to -1 (ERROR) for the current OID
       *
       * @throws Exception
       */
      void markError() {
        publish(this->rawTopic, RawItemStatus{ EVA_ITEM_STATUS_ERROR });
      }
      /**
       * Sets item state to status=1 (OK) and the specified value
       *
       * @tparam T item value kind
       *
       * @param value item value
       *
       * @throws Exception
       */
      template<typename T> void setState(T value) {
        publish(this->rawTopic, RawItemState<T>{ EVA_ITEM_STATUS_OK, value });
      }
      /** item kind */
      vars::ItemKind kind;
      /** OID as string */
      string i;
      /** OID as path */
      string path;
      /** item raw state bus tipic for the current OID */
      string rawTopic;
      bool operator==(const OID& other) const {
        return this->i==other.i;
      }
    private:
      void parse(string s, bool fromPath) {
        char sep = fromPath?'/':':';
        size_t pos = s.find(sep);
        if (pos == string::npos) {
          string opk = fromPath?"oid path":"oid";
          throw invalid_argument("invalid " + opk + ": " + s);
        }
        string sKind = s.substr(0, pos);
        auto i = vars::itemMapK.find(sKind);
        if (i == vars::itemMapK.end()) {
          throw invalid_argument("invalid oid kind: " + s);
        }
        string full_id = s.substr(pos + 1);
        this->kind = i->second;
        this->i = fromPath?sKind+":"+full_id:s;
        this->path = fromPath?s:sKind+"/"+full_id;
        this->rawTopic = EVA_RAW_STATE_TOPIC + this->path;
      }
      size_t pos;
  };

  ostream& operator<<(ostream &strm, const OID &o) {
    return strm << o.i;
  }

  thread_local stringstream result_buf;

  size_t rp() {
    return result_buf.tellp();
  }

  /**
   * SDK controller ("action") functions
   */
  namespace controller {

    using namespace std;

    typedef uint8_t uuidBuf[16];

    struct BusActionStatusShort {
      uint8_t status;
      uuidBuf uuid;

      MSGPACK_DEFINE_MAP(uuid, status)
    };

    struct BusActionStatusCompleted {
      uint8_t status;
      string out;
      int8_t exitcode;
      uuidBuf uuid;

      MSGPACK_DEFINE_MAP(uuid, status, out, exitcode)
    };

    struct BusActionStatusError {
      uint8_t status;
      string err;
      int8_t exitcode;
      uuidBuf uuid;

      MSGPACK_DEFINE_MAP(uuid, status, err, exitcode)
    };

    /**
     * Params of unit action
     *
     * @tparam V value kind
     */
    template <typename V> struct UnitActionParams {
      V value;

      MSGPACK_DEFINE_MAP(value)
    };

    /**
     * Action payload
     *
     * @tparam T action params kind (e.g. UnitActionParams)
     */
    template <typename T> struct ActionData {
      uuidBuf uuid;
      string i;
      uint32_t timeout;
      uint8_t priority;
      T params;

      MSGPACK_DEFINE_MAP(uuid,i,timeout,priority,params)
    };

    struct ActionDataOID {
      string i;

      MSGPACK_DEFINE_MAP(i)
    };

    /**
     * Action class
     *
     * @tparam T action parameters kind (e.g. UnitActionParams)
     */
    template <typename T> class Action {
      public:
        /**
         * Constructs a class from ActionData payload
         *
         * @param a payload
         */
        Action(ActionData<T> a) {
          copy(begin(a.uuid), end(a.uuid), begin(this->uuid));
          this->oid = a.i;
          this->timeout = chrono::microseconds(a.timeout);
          this->priority = a.priority;
          this->params = a.params;
          this->topic = string(EVA_ACTION_STATUS_TOPIC) + this->oid.path;
        }
        /**
         * Mark the action pending
         *
         * @throws Exception
         */
        void markPending() {
          this->markAction(EVA_ACTION_STATUS_PENDING);
        }
        /**
         * Mark the action running
         *
         * @throws Exception
         */
        void markRunning() {
          this->markAction(EVA_ACTION_STATUS_RUNNING);
        }
        /**
         * Mark the action completed
         *
         * @param out action output
         *
         * @throws Exception
         */
        void markCompleted(string out) {
          BusActionStatusCompleted a = { EVA_ACTION_STATUS_COMPLETED, out, 0 };
          copy(begin(this->uuid), end(this->uuid), begin(a.uuid));
          publish<BusActionStatusCompleted>(this->topic, a);
        }
        /**
         * Mark the action failed
         *
         * @param err action error
         * @param exitcode error exit code (custom)
         *
         * @throws Exception
         */
        void markFailed(string err, int8_t exitcode) {
          BusActionStatusError a { EVA_ACTION_STATUS_FAILED, err, exitcode };
          copy(begin(this->uuid), end(this->uuid), begin(a.uuid));
          publish<BusActionStatusError>(this->topic, a);
        }
        /**
         * Mark the action canceled
         *
         * @throws Exception
         */
        void markCanceled() {
          this->markAction(EVA_ACTION_STATUS_CANCELED);
        }
        /**
         * Mark the action terminated
         *
         * @throws Exception
         */
        void markTerminated() {
          this->markAction(EVA_ACTION_STATUS_TERMINATED);
        }
        /** action uuid */
        uuidBuf uuid;
        /** action OID */
        OID oid;
        /** action timeout */
        chrono::microseconds timeout;
        /** action priority */
        uint8_t priority;
        /** action parameters */
        T params;
      private:
        string topic;
        void markAction(uint8_t status) {
          BusActionStatusShort a { status };
          copy(begin(this->uuid), end(this->uuid), begin(a.uuid));
          publish<BusActionStatusShort>(this->topic, a);
        }
    };

  };

  struct SvcMethodParam {
    bool required;

    MSGPACK_DEFINE_MAP(required);
  };

  struct SvcMethod {
    string description;
    map<string, SvcMethodParam> params;

    MSGPACK_DEFINE_MAP(description, params);
  };

  /**
   * Service method information
   */
  class ServiceMethod {
    public:
      /**
       * @param name service method name
       */
      ServiceMethod(string name) {
        this->name = name;
        this->description= "";
      }
      /**
       * @param name service method name
       * @param description service method description
       */
      ServiceMethod(string name, string description) {
        this->name = name;
        this->description= description;
      }
      /**
       * Add a required parameter
       *
       * @param name parameter name
       */
      ServiceMethod required(string name) {
        this->params.insert(pair<string, SvcMethodParam>(name, SvcMethodParam { required: true }));
        return *this;
      }
      /**
       * Add an optional parameter
       *
       * @param name parameter name
       */
      ServiceMethod optional(string name) {
        this->params.insert(pair<string, SvcMethodParam>(name, SvcMethodParam { required: false }));
        return *this;
      }
      string name;
      string description;
      map<string, SvcMethodParam> params;
  };

  /**
   * Service info class
   */
  struct ServiceInfo {
    /**
     * @param author service author
     * @param version service version
     * @param description service description
     */
    ServiceInfo(string author, string version, string description) {
      this->author = author;
      this->version = version;
      this->description = description;
    }
    /**
     * Add a method information. The method information is optional and can be
     * omitted for some methods.
     *
     * @param method service method to add
     */
    ServiceInfo addMethod(ServiceMethod method) {
      methods.insert(pair<string, SvcMethod>(method.name, SvcMethod {method.description, method.params}));
      return *this;
    }
    string author;
    string version;
    string description;
    map<string, SvcMethod> methods;

    MSGPACK_DEFINE_MAP(author, version, description, methods);
  };

  /**
   * Unpacks the initial payload
   *
   * @tparam T Service configuration structure
   *
   * @param buf input FFI buffer
   *
   * @return the payload unpacked
   *
   * @throws MessagePack exceptions
   */
  template <typename T> vars::Initial<T> unpackInitial(EvaFFIBuffer *buf) {
    msgpack::object_handle oh = msgpack::unpack(buf->data, buf->len);
    msgpack::object const& obj = oh.get();
    auto p = obj.as<vars::Initial<T>>();
    return p;
  }

  /**
   * Bus frame
   */
  class Frame {
    public:
      /**
       * Constructs a frame from a raw FFI frame buffer
       */
      Frame(EvaFFIFrame* r) {
        this->r = r;
      }
      /**
       * @return frame sender (primary client id)
       */
      string primary_sender() {
        return string(this->r->primary_sender);
      }
      /**
       * @return frame topic (empty for non pub-sub frames)
       */
      string topic() {
        return string(this->r->topic);
      }
      /**
       * @return if the frame has payload
       */
      bool hasPayload() {
        return this->r->payload_size > 0;
      }
      /**
       * @return MessagePack object handle for the unpacked payload
       *
       * @throws MessagePack exceptions
       */
      msgpack::object_handle unpack() {
        msgpack::object_handle oh = msgpack::unpack(this->r->payload, this->r->payload_size);
        return oh;
      }
    private:
      EvaFFIFrame* r;
  };

  /**
   * Incoming RPC event
   */
  class RpcEvent {
    public:
      /**
       * Constructs an event from a raw FFI event buffer
       */
      RpcEvent(EvaFFIRpcEvent* r) {
        this->r = r;
      }
      /**
       * @return event sender (primary client id)
       */
      string primary_sender() {
        return string(this->r->primary_sender);
      }
      /**
       * @return event method (as string)
       */
      string parse_method() {
        return string(this->r->method, this->r->method_size);
      }
      /**
       * @return if the call has parametes payload
       */
      bool hasPayload() {
        return this->r->payload_size > 0;
      }
      /**
       * @return MessagePack object handle for the unpacked parameters payload
       *
       * @throws MessagePack exceptions
       */
      msgpack::object_handle unpack() {
        msgpack::object_handle oh = msgpack::unpack(this->r->payload, this->r->payload_size);
        return oh;
      }
      /**
       * Get unit action OID (for "action" method called)
       *
       * @return unit OID
       *
       * @throws MessagePack exceptions
       */
      OID asUnitActionOID() {
        if (this->hasPayload()) {
          msgpack::object_handle oh=this->unpack();
          msgpack::object const& obj = oh.get();
          auto a = obj.as<controller::ActionDataOID>();
          return OID(a.i);
        } else {
          throw Exception(EVA_ERR_CODE_INVALID_PARAMS);
        }
      }
      /**
       * Get unit action (for "action" method called)
       *
       * @tparam T action value kind for UnitActionParams
       *
       * @return action object
       *
       * @throws MessagePack exceptions
       */
      template <typename T>controller::Action<controller::UnitActionParams<T>> asUnitAction() {
        if (this->hasPayload()) {
          msgpack::object_handle oh=this->unpack();
          msgpack::object const& obj = oh.get();
          auto a = obj.as<controller::ActionData<controller::UnitActionParams<T>>>();
          return a;
        } else {
          throw Exception(EVA_ERR_CODE_INVALID_PARAMS);
        }
      }
    private:
      EvaFFIRpcEvent* r;
  };

  /**
   * Result of an outgoing RPC call
   */
  class RpcResult {
    public:
      RpcResult(int32_t res) {
        if (res > 0) {
          size_t size = (size_t)res;
          char* data = (char *)malloc(size);
          this->buf = EvaFFIBuffer { size, data, size };
          this->code = 0;
          this->size = size;
          this->getCallResult();
          this->data = data;
        } else {
          this->code = (int16_t)res;
          this->size = 0;
        }
      }
      /**
       * @return if the result has payload
       */
      bool hasPayload() {
        return this->size > 0;
      }
      /**
       * @return MessagePack object handle for the unpacked payload
       *
       * @throws MessagePack exceptions
       */
      msgpack::object_handle unpack() {
        msgpack::object_handle oh = msgpack::unpack(this->buf.data, this->buf.len);
        return oh;
      }
      ~RpcResult() {
        free(this->data);
      }
      int16_t code;
      size_t size;
      EvaFFIBuffer buf;
    private:
      void getCallResult() {
        int16_t code = svc_op_fn(EVA_FFI_SVC_OP_GET_RPC_RESULT, &this->buf);
        if (code != EVA_OK) {
          this->size = 0;
          this->code = EVA_ERR_CODE_CORE_ERROR;
        }
      }
      char* data;
  };

  /**
   * Performs an outgoing RPC call
   *
   * @tparam T call payload kind, must be MessagePack-serializable
   *
   * @param target call target
   * @param method call method
   * @param params call parameters
   *
   * @return RpcResult
   *
   * @throws Exception
   */
  template <typename T> RpcResult rpcCall(string target, string method, T params) {
    stringstream ss;
    ss << target;
    ss.put(0);
    ss << method;
    ss.put(0);
    msgpack::pack(ss, params);
    int32_t res = svcOpSS(EVA_FFI_SVC_OP_RPC_CALL, ss);
    c2e(res);
    RpcResult rpc_result = RpcResult(res);
    return rpc_result;
  }

  /**
   * Performs an outgoing RPC call
   *
   * @param target call target
   * @param method call method
   *
   * @return RpcResult
   *
   * @throws Exception
   */
  RpcResult rpcCall(string target, string method) {
    stringstream ss;
    ss << target;
    ss.put(0);
    ss << method;
    int32_t res = svcOpSS(EVA_FFI_SVC_OP_RPC_CALL, ss);
    c2e(res);
    RpcResult rpc_result = RpcResult(res);
    return rpc_result;
  }

  /**
   * Packs local function result into FFI buffer
   *
   * @tparam T call payload kind, must be MessagePack-serializable
   *
   * @param payload result payload
   *
   * @throws Exception
   */
  template<typename T> int32_t result(T payload) {
    msgpack::pack(result_buf, payload);
    return result_buf.tellp();
  }

  /**
   * Asks the launcher to terminate the service
   *
   * @throws Exception
   */
  void terminate() {
    c2e(svcOp(EVA_FFI_SVC_OP_TERMINATE));
  }

  /**
   * Asks the launcher to panic (immediately terminate) the service on a
   * critical error
   *
   * @throws Exception
   */
  void poc() {
    c2e(svcOp(EVA_FFI_SVC_OP_POC));
  }

  struct coreStatus {
    bool active = false;

    MSGPACK_DEFINE_MAP(active);
  };

  /**
   * Is the node core active
   *
   * @throws Exception
   */
  bool coreActive() {
    RpcResult r = rpcCall("eva.core", "test");
    msgpack::object_handle oh = r.unpack();
    msgpack::object const& obj = oh.get();
    auto p = obj.as<coreStatus>();
    return p.active;
  }

  /**
   * Waits until the node core become active
   *
   * @throws Exception
   */
  void waitCore() {
    while (!coreActive()) {
      this_thread::sleep_for(vars::sleepStep);
    }
  }

  /**
   * Waits until the node core become active
   *
   * @param timeout max wait timeout (throws exception after)
   *
   * @throws Exception
   */
  void waitCore(chrono::milliseconds timeout) {
    const auto waitUntil = chrono::steady_clock::now() + timeout;
    while (!coreActive()) {
      this_thread::sleep_for(vars::sleepStep);
      if (chrono::steady_clock::now() >= waitUntil) {
        throw Exception(EVA_ERR_CODE_TIMEOUT);
      }
    }
  }

  /**
   * SDK bus logging
   */
  namespace log {

    /**
     * Sends a trace-level log message
     *
     * @param message message to send
     */
    void trace(string message) {
      stringstream ss;
      ss << message;
      svcOpSS(EVA_FFI_SVC_OP_LOG_TRACE, ss);
    }

    /**
     * Sends a debug-level log message
     *
     * @param message message to send
     */
    void debug(string message) {
      stringstream ss;
      ss << message;
      svcOpSS(EVA_FFI_SVC_OP_LOG_DEBUG, ss);
    }

    /**
     * Sends an info-level log message
     *
     * @param message message to send
     */
    void info(string message) {
      stringstream ss;
      ss << message;
      svcOpSS(EVA_FFI_SVC_OP_LOG_INFO, ss);
    }

    /**
     * Sends a warn-level log message
     *
     * @param message message to send
     */
    void warn(string message) {
      stringstream ss;
      ss << message;
      svcOpSS(EVA_FFI_SVC_OP_LOG_WARN, ss);
    }

    /**
     * Sends an error-level log message
     *
     * @param message message to send
     */
    void error(string message) {
      stringstream ss;
      ss << message;
      svcOpSS(EVA_FFI_SVC_OP_LOG_ERROR, ss);
    }

  }

  namespace internal {

    extern "C" {

      int16_t eva_svc_get_result(struct EvaFFIBuffer *ebuf) {
        size_t size = eva::result_buf.tellp();
        if (size > ebuf->max) return EVA_ERR_CODE_ABORTED;
        ebuf->len = size;
        memcpy(ebuf->data, result_buf.str().c_str(), size);
        result_buf.str(string());
        return EVA_OK;
      }

      int16_t eva_svc_set_op_fn(uint16_t abi_version, int32_t (*f)(int16_t, struct EvaFFIBuffer*)) {
        if (abi_version != ABI_VERSION) {
          return EVA_ERR_CODE_UNSUPPORTED;
        }
        svc_op_fn = f;
        return EVA_OK;
      }

    }

  };

}

#endif
