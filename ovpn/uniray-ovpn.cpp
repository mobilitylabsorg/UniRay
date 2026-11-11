// uniray-ovpn: an OpenVPN client (the OpenVPN 3 core) without a TUN device.
// uniray-engine runs it as its child and puts the tunnel behind Xray: the
// core reads and writes IP packets on a socket the engine holds the other end
// of, and the engine's userspace stack (gVisor) turns them into connections.
//
// Usage: uniray-ovpn --config <file.ovpn> --packet-fd <fd>
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
// it reads and writes like a TUN's file descriptor.

// The core's TUN abstraction on every platform, not only Android and iOS.
#define USE_TUN_BUILDER
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
#include <unistd.h>

using namespace openvpn;

namespace {

std::mutex out_mutex;

void say(const std::string &line)
{
    std::lock_guard<std::mutex> lock(out_mutex);
    std::cout << line << std::endl;
}

class Client : public ClientAPI::OpenVPNClient
{
  public:
    explicit Client(int packet_fd)
        : packet_fd(packet_fd)
    {
    }

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
        return ::dup(packet_fd);
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
    TunBuilderCapture::Ptr capture;
};

[[noreturn]] void fail(const std::string &message)
{
    say("error " + message);
    std::exit(1);
}

} // namespace

int main(int argc, char *argv[])
{
    std::string config_path;
    int packet_fd = -1;
    for (int i = 1; i + 1 < argc; i += 2)
    {
        const std::string flag = argv[i];
        if (flag == "--config")
            config_path = argv[i + 1];
        else if (flag == "--packet-fd")
            packet_fd = std::atoi(argv[i + 1]);
    }
    if (config_path.empty() || packet_fd < 0)
        fail("usage: uniray-ovpn --config <file.ovpn> --packet-fd <fd>");

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

    ClientAPI::OpenVPNClientHelper helper;
    Client client(packet_fd);

    ClientAPI::Config config;
    config.content = profile.str();
    // The engine reconnects on its own terms; the core retries within a session.
    config.connTimeout = 0;
    config.retryOnAuthFailed = false;
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
    if (status.error)
        fail(status.status + (status.message.empty() ? "" : ": " + status.message));
    return client.failed ? 1 : 0;
}
