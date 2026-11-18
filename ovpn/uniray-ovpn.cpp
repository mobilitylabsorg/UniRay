// uniray-ovpn: an OpenVPN client (the OpenVPN 3 core) without a TUN device.
// uniray-engine runs it as its child and puts the tunnel behind Xray: the
// core reads and writes IP packets on a socket the engine holds the other end
// of, and the engine's userspace stack (gVisor) turns them into connections.
//
// Usage: uniray-ovpn --config <file.ovpn> (--packet-fd <fd> | --packet-udp <port>)
//                    [--server <address> --port <port> [--proto udp|tcp]]
//
// --packet-fd (Unix): a datagram socket inherited from the engine.
// --packet-udp (Windows, which has no socketpair to inherit): the engine's UDP
// port on 127.0.0.1. The first datagram is the token (a third line on stdin);
// the engine takes packets from that sender only, and sends to it.
//
// --server connects there instead of the profile's remote: the address the
// app resolved it to, or a loopback port where Xray forwards to the server
// through another server in front of it.
//
// - The profile is read once and removed (it may hold keys).
// - The credentials come on stdin, a line each: the username, the password.
//   Nothing secret goes on the command line, which other users can read.
// - It reports on stdout, a line each, what the engine needs:
//     tun <mtu>            then the tunnel's settings, until "established":
//     address <ip>/<prefix>
//     route <net>/<prefix>
//     dns <ip>
//     domain <suffix>
//     established
//     connected            the tunnel is up (again)
//     reconnecting
//     error <message>      fatal: it exits after this
//   The log goes to stderr.
// - It exits when stdin closes: the engine is gone.
//
// The core asks for a TUN through TunBuilderBase (USE_TUN_BUILDER, as on
// Android and iOS); tun_builder_establish hands it the packet socket, which
// it reads and writes like a TUN's file descriptor. On Windows the core's TUN
// client for that wants a POSIX descriptor, so there PacketTun, given to the
// core as an external TUN factory, does the same on the UDP socket.

// The core's TUN abstraction on every platform, not only Android and iOS.
#define USE_TUN_BUILDER
#if defined(_WIN32)
#define OPENVPN_EXTERNAL_TUN_FACTORY
#endif
#include <client/ovpncli.cpp>
#include <openvpn/tun/builder/capture.hpp>

#include <atomic>
#include <cstdio>
#include <fstream>
#include <iostream>
#include <mutex>
#include <sstream>
#include <string>
#include <thread>
#if !defined(_WIN32)
#include <unistd.h>
#endif

using namespace openvpn;

namespace {

std::mutex out_mutex;

void say(const std::string &line)
{
    std::lock_guard<std::mutex> lock(out_mutex);
    std::cout << line << std::endl;
}

#if defined(_WIN32)
// The tunnel's packets on a UDP socket to the engine's port on 127.0.0.1, one
// per datagram, as the TUN builder's client does on a descriptor. The core
// makes one for each connection; each greets the engine with the token.
class PacketTun : public TunClient
{
  public:
    typedef RCPtr<PacketTun> Ptr;

    PacketTun(openvpn_io::io_context &io_context, TunClientParent &parent, const ExternalTun::Config &config, TunBuilderBase *builder, unsigned short port, const std::string &token)
        : io_context(io_context), parent(parent), config(config), builder(builder), port(port), token(token), state(new TunProp::State())
    {
    }

    void tun_start(const OptionList &opt, TransportClient &transcli, CryptoDCSettings &) override
    {
        if (socket)
            return;
        try
        {
            if (!builder->tun_builder_new())
                throw Exception("tun_builder_new failed");
            parent.tun_pre_tun_config();
            TunProp::configure_builder(builder, state.get(), config.stats.get(), transcli.server_endpoint_addr(), config.tun_prop, opt, nullptr, false);

            const openvpn_io::ip::address loopback = openvpn_io::ip::address_v4::loopback();
            socket.reset(new openvpn_io::ip::udp::socket(io_context));
            socket->open(openvpn_io::ip::udp::v4());
            socket->set_option(openvpn_io::socket_base::receive_buffer_size(4 << 20));
            socket->set_option(openvpn_io::socket_base::send_buffer_size(4 << 20));
            socket->bind(openvpn_io::ip::udp::endpoint(loopback, 0));
            socket->connect(openvpn_io::ip::udp::endpoint(loopback, port));
            socket->send(openvpn_io::buffer(token));

            // Reports the settings to the engine.
            builder->tun_builder_establish();
            queue_read();
            parent.tun_connected();
        }
        catch (const std::exception &e)
        {
            stop();
            parent.tun_error(Error::TUN_SETUP_FAILED, e.what());
        }
    }

    bool tun_send(BufferAllocated &buf) override
    {
        if (halt || !socket)
            return false;
        openvpn_io::error_code error;
        socket->send(buf.const_buffer(), 0, error);
        if (config.stats && !error)
        {
            config.stats->inc_stat(SessionStats::TUN_BYTES_OUT, buf.size());
            config.stats->inc_stat(SessionStats::TUN_PACKETS_OUT, 1);
        }
        // A full buffer drops the packet, as a TUN would.
        return !error;
    }

    std::string tun_name() const override
    {
        return "uniray";
    }

    std::string vpn_ip4() const override
    {
        return state->vpn_ip4_addr.specified() ? state->vpn_ip4_addr.to_string() : "";
    }

    std::string vpn_ip6() const override
    {
        return state->vpn_ip6_addr.specified() ? state->vpn_ip6_addr.to_string() : "";
    }

    std::string vpn_gw4() const override
    {
        return state->vpn_ip4_gw.specified() ? state->vpn_ip4_gw.to_string() : "";
    }

    std::string vpn_gw6() const override
    {
        return state->vpn_ip6_gw.specified() ? state->vpn_ip6_gw.to_string() : "";
    }

    int vpn_mtu() const override
    {
        return state->mtu;
    }

    void set_disconnect() override
    {
    }

    void stop() override
    {
        if (halt)
            return;
        halt = true;
        if (socket)
        {
            openvpn_io::error_code ignored;
            socket->close(ignored);
        }
    }

  private:
    void queue_read()
    {
        (*config.frame)[Frame::READ_TUN].prepare(incoming);
        socket->async_receive((*config.frame)[Frame::READ_TUN].mutable_buffer(incoming),
                              [self = Ptr(this)](const openvpn_io::error_code &error, const size_t size)
                              {
            OPENVPN_ASYNC_HANDLER;
            self->received(error, size);
        });
    }

    void received(const openvpn_io::error_code &error, const size_t size)
    {
        if (halt)
            return;
        if (error)
        {
            parent.tun_error(Error::TUN_READ_ERROR, "packet socket: " + error.message());
            return;
        }
        incoming.set_size(size);
        if (config.stats)
        {
            config.stats->inc_stat(SessionStats::TUN_BYTES_IN, size);
            config.stats->inc_stat(SessionStats::TUN_PACKETS_IN, 1);
        }
        parent.tun_recv(incoming);
        queue_read();
    }

    openvpn_io::io_context &io_context;
    TunClientParent &parent;
    ExternalTun::Config config;
    TunBuilderBase *builder;
    unsigned short port;
    std::string token;
    TunProp::State::Ptr state;
    std::unique_ptr<openvpn_io::ip::udp::socket> socket;
    BufferAllocated incoming;
    bool halt = false;
};

class PacketTunFactory : public TunClientFactory
{
  public:
    PacketTunFactory(const ExternalTun::Config &config, TunBuilderBase *builder, unsigned short port, const std::string &token)
        : config(config), builder(builder), port(port), token(token)
    {
    }

    TunClient::Ptr new_tun_client_obj(openvpn_io::io_context &io_context, TunClientParent &parent, TransportClient *) override
    {
        return TunClient::Ptr(new PacketTun(io_context, parent, config, builder, port, token));
    }

  private:
    ExternalTun::Config config;
    TunBuilderBase *builder;
    unsigned short port;
    std::string token;
};
#endif

class Client : public ClientAPI::OpenVPNClient
{
  public:
    // packet_fd: the inherited socket (Unix); packet_port and token: the
    // engine's UDP port and the greeting (Windows).
    Client(int packet_fd, unsigned short packet_port, const std::string &token)
        : packet_fd(packet_fd), packet_port(packet_port), token(token)
    {
    }

#if defined(_WIN32)
    TunClientFactory *new_tun_factory(const ExternalTun::Config &config, const OptionList &) override
    {
        return new PacketTunFactory(config, this, packet_port, token);
    }
#endif

    bool tun_builder_new() override
    {
        capture.reset(new TunBuilderCapture());
        return capture->tun_builder_set_mtu(1500);
    }

    bool tun_builder_set_mtu(int mtu) override
    {
        return capture->tun_builder_set_mtu(mtu);
    }

    bool tun_builder_add_address(const std::string &address, int prefix_length, const std::string &gateway, bool ipv6, bool net30) override
    {
        return capture->tun_builder_add_address(address, prefix_length, gateway, ipv6, net30);
    }

    bool tun_builder_reroute_gw(bool ipv4, bool ipv6, unsigned int flags) override
    {
        // Not ours to take: Xray routes the rest of the traffic.
        return true;
    }

    bool tun_builder_add_route(const std::string &address, int prefix_length, int metric, bool ipv6) override
    {
        return capture->tun_builder_add_route(address, prefix_length, metric, ipv6);
    }

    bool tun_builder_exclude_route(const std::string &address, int prefix_length, int metric, bool ipv6) override
    {
        return true;
    }

    bool tun_builder_add_dns_options(const DnsOptions &dns) override
    {
        return capture->tun_builder_add_dns_options(dns);
    }

    bool tun_builder_add_dns_server(const std::string &address, bool ipv6) override
    {
        return capture->tun_builder_add_dns_server(address, ipv6);
    }

    bool tun_builder_add_search_domain(const std::string &domain) override
    {
        return capture->tun_builder_add_search_domain(domain);
    }

    bool tun_builder_set_session_name(const std::string &name) override
    {
        return true;
    }

    bool tun_builder_set_remote_address(const std::string &address, bool ipv6) override
    {
        return true;
    }

    // The settings go to the engine, which brings up its stack with them; the
    // core gets a copy of the packet socket (it closes its copy on teardown).
    int tun_builder_establish() override
    {
        say("tun " + std::to_string(capture->mtu > 0 ? capture->mtu : 1500));
        for (const auto &address : capture->tunnel_addresses)
            say("address " + address.address + "/" + std::to_string(address.prefix_length));
        for (const auto &route : capture->add_routes)
            say("route " + route.address + "/" + std::to_string(route.prefix_length));
        for (const auto &server : capture->dns_servers)
            say("dns " + server.address);
        for (const auto &[priority, server] : capture->dns_options.servers)
        {
            for (const auto &address : server.addresses)
                say("dns " + address.address.to_string());
            for (const auto &domain : server.domains)
                say("domain " + domain.domain);
        }
        for (const auto &domain : capture->search_domains)
            say("domain " + domain.domain);
        for (const auto &domain : capture->dns_options.search_domains)
            say("domain " + domain.domain);
        say("established");
#if defined(_WIN32)
        return 0; // PacketTun has the socket
#else
        return ::dup(packet_fd);
#endif
    }

    bool tun_builder_persist() override
    {
        return false;
    }

    void tun_builder_teardown(bool disconnect) override
    {
    }

    bool socket_protect(openvpn_io::detail::socket_type socket, std::string remote, bool ipv6) override
    {
        return true;
    }

    bool pause_on_connection_timeout() override
    {
        return false;
    }

    void event(const ClientAPI::Event &ev) override
    {
        if (ev.name == "CONNECTED")
            say("connected");
        else if (ev.name == "RECONNECTING")
            say("reconnecting");
        else if (ev.fatal)
        {
            failed = true;
            say("error " + ev.name + (ev.info.empty() ? "" : ": " + ev.info));
            // The core reports AUTH_FAILED and the like but keeps connect()
            // waiting; stop() is posted to its loop, from outside it.
            std::thread([this] { stop(); }).detach();
        }
        std::cerr << "ovpn: event " << ev.name << (ev.info.empty() ? "" : " " + ev.info) << std::endl;
    }

    void acc_event(const ClientAPI::AppCustomControlMessageEvent &) override
    {
    }

    void log(const ClientAPI::LogInfo &info) override
    {
        std::cerr << "ovpn: " << info.text;
        if (info.text.empty() || info.text.back() != '\n')
            std::cerr << std::endl;
    }

    void external_pki_cert_request(ClientAPI::ExternalPKICertRequest &req) override
    {
        req.error = true;
        req.errorText = "external PKI is not supported";
    }

    void external_pki_sign_request(ClientAPI::ExternalPKISignRequest &req) override
    {
        req.error = true;
        req.errorText = "external PKI is not supported";
    }

    // A fatal event ended the session (AUTH_FAILED and the like).
    std::atomic<bool> failed{false};

  private:
    int packet_fd;
    unsigned short packet_port;
    std::string token;
    TunBuilderCapture::Ptr capture;
};

// Ends the process without running destructors: the thread watching stdin
// still holds the client.
[[noreturn]] void quit(int code)
{
    std::cout.flush();
    std::cerr.flush();
    std::_Exit(code);
}

[[noreturn]] void fail(const std::string &message)
{
    say("error " + message);
    quit(1);
}

} // namespace

int main(int argc, char *argv[])
{
    std::string config_path, server, port, proto;
    int packet_fd = -1, packet_port = 0;
    for (int i = 1; i + 1 < argc; i += 2)
    {
        const std::string flag = argv[i];
        if (flag == "--config")
            config_path = argv[i + 1];
        else if (flag == "--packet-fd")
            packet_fd = std::atoi(argv[i + 1]);
        else if (flag == "--packet-udp")
            packet_port = std::atoi(argv[i + 1]);
        else if (flag == "--server")
            server = argv[i + 1];
        else if (flag == "--port")
            port = argv[i + 1];
        else if (flag == "--proto")
            proto = argv[i + 1];
    }
#if defined(_WIN32)
    if (config_path.empty() || packet_port <= 0 || packet_port > 65535)
        fail("usage: uniray-ovpn --config <file.ovpn> --packet-udp <port>");
#else
    if (config_path.empty() || packet_fd < 0)
        fail("usage: uniray-ovpn --config <file.ovpn> --packet-fd <fd>");
#endif

    std::ifstream file(config_path);
    if (!file)
        fail("cannot read the profile");
    std::stringstream profile;
    profile << file.rdbuf();
    file.close();
    std::remove(config_path.c_str());

    std::string username, password;
    std::getline(std::cin, username);
    std::getline(std::cin, password);
    std::string token;
    if (packet_port > 0)
        std::getline(std::cin, token);
    if (packet_port > 0 && token.empty())
        fail("no token for the packet socket");

    ClientAPI::OpenVPNClientHelper helper;
    Client client(packet_fd, static_cast<unsigned short>(packet_port), token);

    ClientAPI::Config config;
    config.content = profile.str();
    // The engine reconnects on its own terms; the core retries within a session.
    config.connTimeout = 0;
    config.retryOnAuthFailed = false;
    config.serverOverride = server;
    config.portOverride = port;
    config.protoOverride = proto;
    const ClientAPI::EvalConfig eval = client.eval_config(config);
    if (eval.error)
        fail("the profile: " + eval.message);
    if (!eval.autologin)
    {
        ClientAPI::ProvideCreds creds;
        creds.username = username;
        creds.password = password;
        const ClientAPI::Status status = client.provide_creds(creds);
        if (status.error)
            fail("the credentials: " + status.message);
    }

    // stdin closing (the engine is gone) stops the session.
    std::thread watcher([&client] {
        std::string line;
        while (std::getline(std::cin, line))
        {
        }
        client.stop();
    });
    watcher.detach();

    const ClientAPI::Status status = client.connect();
    if (client.failed)
        quit(1);
    if (status.error)
        fail(status.status + (status.message.empty() ? "" : ": " + status.message));
    quit(0);
}
