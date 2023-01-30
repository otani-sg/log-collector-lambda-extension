FROM amazonlinux:2

RUN yum install -y shadow-utils gcc zip pkg-config openssl-devel  && rm -rf /var/cache/yum/* && yum clean all
RUN useradd --create-home --shell /bin/bash dev
USER dev
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh /dev/stdin -y
ENV PATH="/home/dev/.cargo/bin:${PATH}"