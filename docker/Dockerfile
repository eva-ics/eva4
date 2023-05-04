from ubuntu:20.04
RUN touch /.eva_container
RUN apt-get update
RUN env DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y tzdata
RUN ln -sf /usr/share/zoneinfo/Etc/UTC /etc/localtime
RUN apt-get update
RUN apt-get install -y --no-install-recommends bash jq curl procps ca-certificates tar gzip adduser
RUN apt-get install -y --no-install-recommends python3 python3-venv
RUN apt-get install  -y --no-install-recommends vim-nox
RUN addgroup --system eva
RUN adduser --system --no-create-home --home /opt/eva4 --ingroup eva eva
RUN curl https://pub.bma.ai/eva4/4.0.1/nightly/eva-4.0.1-2023050101-x86_64-ubuntu20.04.tgz | tar xzf - -C /opt
RUN mv -f /opt/eva-4.0.1 /opt/eva4
RUN curl https://pub.bma.ai/eva4/4.0.1/nightly/update-2023050101.sh -o /opt/eva4/update.sh
RUN /opt/eva4/prepare
RUN /opt/eva4/sbin/venvmgr build
RUN /opt/eva4/sbin/venvmgr add eva-shell
RUN ln -sf /opt/eva4/bin/eva /usr/local/bin/eva
RUN echo '[{"level":"info","output":"console"},{"level":"info","output":"bus"},{"keep":86400,"level":"info","max_records":1000000,"output":"memory"}]' | /opt/eva4/sbin/eva-registry-cli set eva/config/logs - --type json
COPY eva-launcher /
CMD [ "/bin/bash", "-c", "/eva-launcher" ]
