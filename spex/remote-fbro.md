# Tips and Tricks

If you also want to silence the underlying issue (Linux's mDNS not re-announcing to the Mac's queries), the most common cause is firewall — try:

## UFW
sudo ufw allow 5353/udp comment 'mDNS'

## Or, if firewalld is in play
sudo firewall-cmd --permanent --add-service=mdns
sudo firewall-cmd --reload