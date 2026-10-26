#define _POSIX_C_SOURCE 200809L
#define BUILDING_WPE__ 1
#define ENABLE_WPE_PLATFORM 1

#include "alloyos_tls_backend.h"

#include <gio/gio.h>
#include <gio/gtlsbackend.h>
#include <gio/gtlsclientconnection.h>
#include <gio/gfilterinputstream.h>
#include <gio/gfilteroutputstream.h>
#include <string.h>

// Rust FFI declarations from liballoy::net
extern "C" {
    typedef void AlloyosTlsSession;
    AlloyosTlsSession* alloyos_tls_create(const char* server_name);
    int alloyos_tls_client_hello(AlloyosTlsSession* session, uint8_t* out_buf, size_t max_len);
    int alloyos_tls_consume_server(AlloyosTlsSession* session, const uint8_t* in_buf, size_t in_len, uint8_t* out_finished, size_t max_finished_len);
    int alloyos_tls_is_ready(const AlloyosTlsSession* session);
    int alloyos_tls_seal(AlloyosTlsSession* session, const uint8_t* plaintext, size_t plaintext_len, uint8_t* out_buf, size_t max_len);
    int alloyos_tls_open(AlloyosTlsSession* session, const uint8_t* in_buf, size_t in_len, uint8_t* out_buf, size_t max_len);
    void alloyos_tls_destroy(AlloyosTlsSession* session);
}

// ----------------------------------------------------------------------------
// AlloyosTlsInputStream
// ----------------------------------------------------------------------------
typedef struct _AlloyosTlsInputStream {
    GFilterInputStream parent_instance;
    AlloyosTlsSession* session;
} AlloyosTlsInputStream;

typedef struct _AlloyosTlsInputStreamClass {
    GFilterInputStreamClass parent_class;
} AlloyosTlsInputStreamClass;

G_DEFINE_TYPE(AlloyosTlsInputStream, alloyos_tls_input_stream, G_TYPE_FILTER_INPUT_STREAM)

static gssize alloyos_tls_input_stream_read(GInputStream* stream, void* buffer, gsize count, GCancellable* cancellable, GError** error)
{
    auto* self = (AlloyosTlsInputStream*)stream;
    GInputStream* base = g_filter_input_stream_get_base_stream(G_FILTER_INPUT_STREAM(stream));
    if (!self->session || !alloyos_tls_is_ready(self->session))
        return 0;

    uint8_t hdr[5];
    gsize bytes_read = 0;
    if (!g_input_stream_read_all(base, hdr, 5, &bytes_read, cancellable, error) || bytes_read < 5)
        return bytes_read;

    uint16_t body_len = (hdr[3] << 8) | hdr[4];
    uint8_t rec[4096];
    if (5 + body_len > sizeof(rec))
        return 0;

    memcpy(rec, hdr, 5);
    if (!g_input_stream_read_all(base, rec + 5, body_len, &bytes_read, cancellable, error) || bytes_read < body_len)
        return 0;

    int plain = alloyos_tls_open(self->session, rec, 5 + body_len, (uint8_t*)buffer, count);
    return plain > 0 ? plain : 0;
}

static void alloyos_tls_input_stream_class_init(AlloyosTlsInputStreamClass* klass)
{
    GInputStreamClass* istream_class = G_INPUT_STREAM_CLASS(klass);
    istream_class->read_fn = alloyos_tls_input_stream_read;
}

static void alloyos_tls_input_stream_init(AlloyosTlsInputStream* self)
{
    self->session = nullptr;
}

static GInputStream* alloyos_tls_input_stream_new(GInputStream* base_stream, AlloyosTlsSession* session)
{
    auto* stream = (AlloyosTlsInputStream*)g_object_new(alloyos_tls_input_stream_get_type(), "base-stream", base_stream, nullptr);
    stream->session = session;
    return G_INPUT_STREAM(stream);
}

// ----------------------------------------------------------------------------
// AlloyosTlsOutputStream
// ----------------------------------------------------------------------------
typedef struct _AlloyosTlsOutputStream {
    GFilterOutputStream parent_instance;
    AlloyosTlsSession* session;
} AlloyosTlsOutputStream;

typedef struct _AlloyosTlsOutputStreamClass {
    GFilterOutputStreamClass parent_class;
} AlloyosTlsOutputStreamClass;

G_DEFINE_TYPE(AlloyosTlsOutputStream, alloyos_tls_output_stream, G_TYPE_FILTER_OUTPUT_STREAM)

static gssize alloyos_tls_output_stream_write(GOutputStream* stream, const void* buffer, gsize count, GCancellable* cancellable, GError** error)
{
    auto* self = (AlloyosTlsOutputStream*)stream;
    GOutputStream* base = g_filter_output_stream_get_base_stream(G_FILTER_OUTPUT_STREAM(stream));
    if (!self->session)
        return 0;

    uint8_t rec[4096];
    if (count + 64 > sizeof(rec))
        return 0;

    int sealed = alloyos_tls_seal(self->session, (const uint8_t*)buffer, count, rec, sizeof(rec));
    if (sealed <= 0)
        return 0;

    gsize written = 0;
    if (!g_output_stream_write_all(base, rec, sealed, &written, cancellable, error))
        return -1;

    return count;
}

static void alloyos_tls_output_stream_class_init(AlloyosTlsOutputStreamClass* klass)
{
    GOutputStreamClass* ostream_class = G_OUTPUT_STREAM_CLASS(klass);
    ostream_class->write_fn = alloyos_tls_output_stream_write;
}

static void alloyos_tls_output_stream_init(AlloyosTlsOutputStream* self)
{
    self->session = nullptr;
}

static GOutputStream* alloyos_tls_output_stream_new(GOutputStream* base_stream, AlloyosTlsSession* session)
{
    auto* stream = (AlloyosTlsOutputStream*)g_object_new(alloyos_tls_output_stream_get_type(), "base-stream", base_stream, nullptr);
    stream->session = session;
    return G_OUTPUT_STREAM(stream);
}

// ----------------------------------------------------------------------------
// AlloyosTlsConnection
// ----------------------------------------------------------------------------
typedef struct _AlloyosTlsConnection {
    GTlsConnection parent_instance;
    GIOStream* base_io_stream;
    GSocketConnectable* server_identity;
    AlloyosTlsSession* session;
    GInputStream* in_stream;
    GOutputStream* out_stream;
    bool handshake_done;
} AlloyosTlsConnection;

typedef struct _AlloyosTlsConnectionClass {
    GTlsConnectionClass parent_class;
} AlloyosTlsConnectionClass;

enum {
    PROP_0,
    PROP_BASE_IO_STREAM,
    PROP_USE_SYSTEM_CERTDB,
    PROP_DATABASE,
    PROP_CERTIFICATE,
    PROP_INTERACTION,
    PROP_PEER_CERTIFICATE,
    PROP_PEER_CERTIFICATE_ERRORS,
    PROP_VALIDATION_FLAGS,
    PROP_SERVER_IDENTITY,
    PROP_USE_SSL3,
    PROP_ACCEPTED_CAS,
};

static void alloyos_tls_client_connection_iface_init(GTlsClientConnectionInterface* iface)
{
    iface->copy_session_state = nullptr;
}

G_DEFINE_TYPE_WITH_CODE(AlloyosTlsConnection, alloyos_tls_connection, G_TYPE_TLS_CONNECTION,
                        G_IMPLEMENT_INTERFACE(G_TYPE_TLS_CLIENT_CONNECTION, alloyos_tls_client_connection_iface_init))

static gboolean alloyos_tls_connection_handshake(GTlsConnection* conn, GCancellable* cancellable, GError** error)
{
    auto* self = (AlloyosTlsConnection*)conn;
    if (self->handshake_done)
        return TRUE;

    if (!self->base_io_stream) {
        g_set_error_literal(error, G_TLS_ERROR, G_TLS_ERROR_MISC, "No base I/O stream for TLS connection");
        return FALSE;
    }

    const char* hostname = "localhost";
    if (self->server_identity && G_IS_NETWORK_ADDRESS(self->server_identity)) {
        hostname = g_network_address_get_hostname(G_NETWORK_ADDRESS(self->server_identity));
    }

    self->session = alloyos_tls_create(hostname);
    if (!self->session) {
        g_set_error_literal(error, G_TLS_ERROR, G_TLS_ERROR_MISC, "Failed to create TLS session");
        return FALSE;
    }

    GOutputStream* base_out = g_io_stream_get_output_stream(self->base_io_stream);
    GInputStream* base_in = g_io_stream_get_input_stream(self->base_io_stream);

    uint8_t ch[1024];
    int ch_len = alloyos_tls_client_hello(self->session, ch, sizeof(ch));
    if (ch_len <= 0) {
        g_set_error_literal(error, G_TLS_ERROR, G_TLS_ERROR_HANDSHAKE, "Failed to build ClientHello");
        return FALSE;
    }

    gsize written = 0;
    if (!g_output_stream_write_all(base_out, ch, ch_len, &written, cancellable, error))
        return FALSE;

    // Read server handshake records
    uint8_t rx[4096];
    gsize bytes_read = 0;
    if (!g_input_stream_read_all(base_in, rx, sizeof(rx), &bytes_read, cancellable, error) || bytes_read == 0) {
        g_set_error_literal(error, G_TLS_ERROR, G_TLS_ERROR_HANDSHAKE, "Server closed connection during handshake");
        return FALSE;
    }

    uint8_t fin[512];
    int fin_len = alloyos_tls_consume_server(self->session, rx, bytes_read, fin, sizeof(fin));
    if (fin_len < 0) {
        g_set_error_literal(error, G_TLS_ERROR, G_TLS_ERROR_BAD_CERTIFICATE, "TLS handshake verification failed");
        return FALSE;
    }

    if (fin_len > 0) {
        if (!g_output_stream_write_all(base_out, fin, fin_len, &written, cancellable, error))
            return FALSE;
    }

    if (!alloyos_tls_is_ready(self->session)) {
        g_set_error_literal(error, G_TLS_ERROR, G_TLS_ERROR_HANDSHAKE, "TLS handshake did not complete");
        return FALSE;
    }

    self->in_stream = alloyos_tls_input_stream_new(base_in, self->session);
    self->out_stream = alloyos_tls_output_stream_new(base_out, self->session);
    self->handshake_done = true;
    return TRUE;
}

static void alloyos_tls_connection_handshake_async(GTlsConnection* conn, int io_priority, GCancellable* cancellable, GAsyncReadyCallback callback, gpointer user_data)
{
    GTask* task = g_task_new(conn, cancellable, callback, user_data);
    g_task_set_priority(task, io_priority);
    GError* error = nullptr;
    gboolean success = alloyos_tls_connection_handshake(conn, cancellable, &error);
    if (success) {
        g_task_return_boolean(task, TRUE);
    } else {
        g_task_return_error(task, error);
    }
    g_object_unref(task);
}

static gboolean alloyos_tls_connection_handshake_finish(GTlsConnection* conn, GAsyncResult* result, GError** error)
{
    (void)conn;
    return g_task_propagate_boolean(G_TASK(result), error);
}

static GInputStream* alloyos_tls_connection_get_input_stream(GIOStream* stream)
{
    auto* self = (AlloyosTlsConnection*)stream;
    return self->in_stream;
}

static GOutputStream* alloyos_tls_connection_get_output_stream(GIOStream* stream)
{
    auto* self = (AlloyosTlsConnection*)stream;
    return self->out_stream;
}

static gboolean alloyos_tls_connection_close(GIOStream* stream, GCancellable* cancellable, GError** error)
{
    auto* self = (AlloyosTlsConnection*)stream;
    if (self->session) {
        alloyos_tls_destroy(self->session);
        self->session = nullptr;
    }
    if (self->in_stream) {
        g_object_unref(self->in_stream);
        self->in_stream = nullptr;
    }
    if (self->out_stream) {
        g_object_unref(self->out_stream);
        self->out_stream = nullptr;
    }
    if (self->base_io_stream) {
        return g_io_stream_close(self->base_io_stream, cancellable, error);
    }
    return TRUE;
}

static void alloyos_tls_connection_set_property(GObject* object, guint prop_id, const GValue* value, GParamSpec* pspec)
{
    auto* self = (AlloyosTlsConnection*)object;
    (void)pspec;
    switch (prop_id) {
    case PROP_BASE_IO_STREAM:
        if (self->base_io_stream)
            g_object_unref(self->base_io_stream);
        self->base_io_stream = G_IO_STREAM(g_value_dup_object(value));
        break;
    case PROP_SERVER_IDENTITY:
        if (self->server_identity)
            g_object_unref(self->server_identity);
        self->server_identity = G_SOCKET_CONNECTABLE(g_value_dup_object(value));
        break;
    default:
        break;
    }
}

static void alloyos_tls_connection_get_property(GObject* object, guint prop_id, GValue* value, GParamSpec* pspec)
{
    auto* self = (AlloyosTlsConnection*)object;
    (void)pspec;
    switch (prop_id) {
    case PROP_BASE_IO_STREAM:
        g_value_set_object(value, self->base_io_stream);
        break;
    case PROP_SERVER_IDENTITY:
        g_value_set_object(value, self->server_identity);
        break;
    default:
        break;
    }
}

static void alloyos_tls_connection_class_init(AlloyosTlsConnectionClass* klass)
{
    GObjectClass* gobject_class = G_OBJECT_CLASS(klass);
    gobject_class->set_property = alloyos_tls_connection_set_property;
    gobject_class->get_property = alloyos_tls_connection_get_property;

    g_object_class_override_property(gobject_class, PROP_BASE_IO_STREAM, "base-io-stream");
    g_object_class_override_property(gobject_class, PROP_USE_SYSTEM_CERTDB, "use-system-certdb");
    g_object_class_override_property(gobject_class, PROP_DATABASE, "database");
    g_object_class_override_property(gobject_class, PROP_CERTIFICATE, "certificate");
    g_object_class_override_property(gobject_class, PROP_INTERACTION, "interaction");
    g_object_class_override_property(gobject_class, PROP_PEER_CERTIFICATE, "peer-certificate");
    g_object_class_override_property(gobject_class, PROP_PEER_CERTIFICATE_ERRORS, "peer-certificate-errors");
    g_object_class_override_property(gobject_class, PROP_VALIDATION_FLAGS, "validation-flags");
    g_object_class_override_property(gobject_class, PROP_SERVER_IDENTITY, "server-identity");
    g_object_class_override_property(gobject_class, PROP_USE_SSL3, "use-ssl3");
    g_object_class_override_property(gobject_class, PROP_ACCEPTED_CAS, "accepted-cas");

    GIOStreamClass* stream_class = G_IO_STREAM_CLASS(klass);
    stream_class->get_input_stream = alloyos_tls_connection_get_input_stream;
    stream_class->get_output_stream = alloyos_tls_connection_get_output_stream;
    stream_class->close_fn = alloyos_tls_connection_close;

    GTlsConnectionClass* conn_class = G_TLS_CONNECTION_CLASS(klass);
    conn_class->handshake = alloyos_tls_connection_handshake;
    conn_class->handshake_async = alloyos_tls_connection_handshake_async;
    conn_class->handshake_finish = alloyos_tls_connection_handshake_finish;
}

static void alloyos_tls_connection_init(AlloyosTlsConnection* self)
{
    self->base_io_stream = nullptr;
    self->server_identity = nullptr;
    self->session = nullptr;
    self->in_stream = nullptr;
    self->out_stream = nullptr;
    self->handshake_done = false;
}

// ----------------------------------------------------------------------------
// AlloyosTlsBackend
// ----------------------------------------------------------------------------
typedef struct _AlloyosTlsBackend {
    GObject parent_instance;
} AlloyosTlsBackend;

typedef struct _AlloyosTlsBackendClass {
    GObjectClass parent_class;
} AlloyosTlsBackendClass;

static void alloyos_tls_backend_iface_init(GTlsBackendInterface* iface);

G_DEFINE_TYPE_WITH_CODE(AlloyosTlsBackend, alloyos_tls_backend, G_TYPE_OBJECT,
                        G_IMPLEMENT_INTERFACE(G_TYPE_TLS_BACKEND, alloyos_tls_backend_iface_init))

static void alloyos_tls_backend_class_init(AlloyosTlsBackendClass* klass)
{
    (void)klass;
}

static void alloyos_tls_backend_init(AlloyosTlsBackend* self)
{
    (void)self;
}

static gboolean alloyos_tls_backend_supports_tls(GTlsBackend* backend)
{
    (void)backend;
    return TRUE;
}

static GType alloyos_tls_backend_get_certificate_type(void)
{
    return G_TYPE_INVALID;
}

static GType alloyos_tls_backend_get_client_connection_type(void)
{
    return alloyos_tls_connection_get_type();
}

static GType alloyos_tls_backend_get_server_connection_type(void)
{
    return G_TYPE_INVALID;
}

static GType alloyos_tls_backend_get_file_database_type(void)
{
    return G_TYPE_INVALID;
}

static GTlsDatabase* alloyos_tls_backend_get_default_database(GTlsBackend* backend)
{
    (void)backend;
    return nullptr;
}

static gboolean alloyos_tls_backend_supports_dtls(GTlsBackend* backend)
{
    (void)backend;
    return FALSE;
}

static GType alloyos_tls_backend_get_dtls_client_connection_type(void)
{
    return G_TYPE_INVALID;
}

static GType alloyos_tls_backend_get_dtls_server_connection_type(void)
{
    return G_TYPE_INVALID;
}

static void alloyos_tls_backend_iface_init(GTlsBackendInterface* iface)
{
    iface->supports_tls = alloyos_tls_backend_supports_tls;
    iface->get_certificate_type = alloyos_tls_backend_get_certificate_type;
    iface->get_client_connection_type = alloyos_tls_backend_get_client_connection_type;
    iface->get_server_connection_type = alloyos_tls_backend_get_server_connection_type;
    iface->get_file_database_type = alloyos_tls_backend_get_file_database_type;
    iface->get_default_database = alloyos_tls_backend_get_default_database;
    iface->supports_dtls = alloyos_tls_backend_supports_dtls;
    iface->get_dtls_client_connection_type = alloyos_tls_backend_get_dtls_client_connection_type;
    iface->get_dtls_server_connection_type = alloyos_tls_backend_get_dtls_server_connection_type;
}

void alloyos_tls_backend_register(void)
{
    static gsize initialized = 0;
    if (g_once_init_enter(&initialized)) {
        g_io_extension_point_implement(
            G_TLS_BACKEND_EXTENSION_POINT_NAME,
            alloyos_tls_backend_get_type(),
            "alloyos-tls",
            100
        );
        g_once_init_leave(&initialized, 1);
    }
}
