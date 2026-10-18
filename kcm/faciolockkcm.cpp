#include <KPluginFactory>
#include <KQuickConfigModule>

#include <QFile>
#include <QProcess>

#include <pwd.h>
#include <unistd.h>

namespace
{
// ponytail: the panel shells out to faciolockctl rather than speaking the socket protocol
// itself. One IPC surface, and the enrolment loop already exists in one place.
// Write a Qt client only if the panel ever needs something the CLI cannot say.
//
// Absolute, because polkit's exec.path annotation names this exact path. A bare name would
// still work -- pkexec resolves PATH first -- but then the action that matches depends on
// $PATH, which is not a thing a privileged prompt should depend on.
const QString Ctl = QStringLiteral("/usr/bin/faciolockctl");
const QString CommonAuth = QStringLiteral("/etc/pam.d/common-auth");
const QString PoseMarker = QStringLiteral("[pose ");
}

class FaciolockKcm : public KQuickConfigModule
{
    Q_OBJECT

    // One signal for all of them. Five NOTIFY signals would let QML re-evaluate five
    // bindings instead of five; nothing here updates often enough to care.
    Q_PROPERTY(QString enrolment READ enrolment NOTIFY changed)
    Q_PROPERTY(QString trouble READ trouble NOTIFY changed)
    Q_PROPERTY(bool pamEnabled READ pamEnabled NOTIFY changed)
    Q_PROPERTY(bool busy READ busy NOTIFY changed)
    Q_PROPERTY(QString pose READ pose NOTIFY changed)
    Q_PROPERTY(QString log READ log NOTIFY changed)

public:
    FaciolockKcm(QObject *parent, const KPluginMetaData &data)
        : KQuickConfigModule(parent, data)
    {
        connect(&m_task, &QProcess::readyReadStandardOutput, this, &FaciolockKcm::readTask);
        connect(&m_task, &QProcess::readyReadStandardError, this, &FaciolockKcm::readTask);
        connect(&m_task, &QProcess::errorOccurred, this, [this](QProcess::ProcessError error) {
            append(error == QProcess::FailedToStart
                ? QStringLiteral("Could not start the authentication helper")
                : QStringLiteral("The authentication helper stopped unexpectedly"));
            refresh();
        });
        connect(&m_task, &QProcess::finished, this, [this] {
            readTask();
            m_pose.clear();
            append(m_task.exitCode() == 0 ? QStringLiteral("done") : QStringLiteral("failed"));
            refresh();
        });
        connect(&m_status, &QProcess::finished, this, &FaciolockKcm::readStatus);
        connect(&m_status, &QProcess::errorOccurred, this, [this] {
            m_trouble = Ctl + QStringLiteral(" is not installed");
            Q_EMIT changed();
        });
        refresh();
    }

    QString enrolment() const
    {
        return m_enrolment;
    }
    QString trouble() const
    {
        return m_trouble;
    }
    bool pamEnabled() const
    {
        return m_pamEnabled;
    }
    bool busy() const
    {
        return m_task.state() != QProcess::NotRunning;
    }
    QString pose() const
    {
        return m_pose;
    }
    QString log() const
    {
        return m_log;
    }

    // No pkexec, and asynchronous. serve::may_ask lets a faciolock-auth member ask Status
    // about itself, so opening the panel costs no password prompt -- but the first connect
    // is socket activation, and a cold faciolockd has taken up to two seconds to answer.
    // Blocking on that would freeze System Settings.
    Q_INVOKABLE void refresh()
    {
        m_pamEnabled = pamProfileEnabled();
        if (m_status.state() == QProcess::NotRunning) {
            m_status.start(Ctl, {QStringLiteral("status")});
        }
        Q_EMIT changed();
    }

    // The name comes from getpwuid, not from QML and not from $USER: pkexec discards the
    // environment ($SUDO_USER included, which is what faciolockctl falls back to), and a
    // panel that took a username from its own UI could be pointed at somebody else.
    Q_INVOKABLE void run(const QString &verb)
    {
        const QString user = me();
        if (busy() || user.isEmpty()) {
            return;
        }
        m_log.clear();
        m_pose.clear();
        QStringList args{Ctl, verb};
        if (verb != QLatin1String("enable") && verb != QLatin1String("disable")) {
            args << user;
        }
        if (verb == QLatin1String("remove")) {
            args << QStringLiteral("--yes");
        }
        m_task.start(QStringLiteral("pkexec"), args);
        Q_EMIT changed();
    }

    // faciolockctl's prompt() blocks on stdin between poses and its prompt carries no
    // newline; the marker line is the cue, and this is the newline it is waiting for.
    Q_INVOKABLE void capture()
    {
        if (m_pose.isEmpty()) {
            return;
        }
        m_task.write("\n");
        m_pose.clear();
        Q_EMIT changed();
    }

    // Closing stdin, not terminate(). After pkexec authorises, the child runs as root and an
    // unprivileged panel signalling it gets EPERM -- the button would do nothing at all.
    // faciolockctl treats a closed stdin as an abandoned enrolment and exits.
    //
    // ponytail: only enrolment reads stdin, so this takes effect at the next pose and is a
    // no-op for the two-second `test`. A real cancel for those needs a privileged helper to
    // do the signalling, which is a boundary this panel deliberately does not have.
    Q_INVOKABLE void cancel()
    {
        m_task.closeWriteChannel();
    }

Q_SIGNALS:
    void changed();

private:
    void readTask()
    {
        m_task.setReadChannel(QProcess::StandardOutput);
        // canReadLine, not readLine-until-empty: a read can deliver half a line, and half of
        // "[pose 1/3] ..." does not start with the marker. It would land in the log, Capture
        // would never appear, and faciolockctl would sit on stdin forever -- the exact hang
        // the marker exists to prevent. This also leaves the newline-less "[Enter] " prompt
        // unread, which is why it never reaches the log.
        while (m_task.canReadLine()) {
            const QString line = QString::fromUtf8(m_task.readLine()).trimmed();
            if (line.startsWith(PoseMarker)) {
                m_pose = line.section(QLatin1Char(']'), 1).trimmed();
            } else if (!line.isEmpty()) {
                append(line);
            }
        }
        const QString errors = QString::fromUtf8(m_task.readAllStandardError()).trimmed();
        if (!errors.isEmpty()) {
            const QString verb = m_task.arguments().value(1);
            append(verb == QLatin1String("enable") || verb == QLatin1String("disable")
                ? QStringLiteral("Could not change face unlock")
                : errors);
        }
        Q_EMIT changed();
    }

    void readStatus()
    {
        const QString out = QString::fromUtf8(m_status.readAllStandardOutput()).trimmed();
        const QString err = QString::fromUtf8(m_status.readAllStandardError()).trimmed();
        // faciolockctl prints "enrolled csr" or "not-enrolled csr" and exits 0; every other
        // state -- no daemon, not in the group -- is stderr and a non-zero exit, which is a
        // thing to show rather than an error to swallow.
        m_enrolment = m_status.exitCode() == 0 ? out.section(QLatin1Char(' '), 0, 0) : QString();
        m_trouble = m_status.exitCode() == 0 ? QString() : err;
        Q_EMIT changed();
    }

    static QString me()
    {
        const passwd *pw = getpwuid(geteuid());
        return pw ? QString::fromLocal8Bit(pw->pw_name) : QString();
    }

    static bool pamProfileEnabled()
    {
        QFile profile(CommonAuth);
        if (!profile.open(QIODevice::ReadOnly)) {
            return false;
        }
        for (const QByteArray &line : profile.readAll().split('\n')) {
            if (line.split('#').first().simplified().split(' ').contains("pam_faciolock.so")) {
                return true;
            }
        }
        return false;
    }

    void append(const QString &line)
    {
        m_log += m_log.isEmpty() ? line : QLatin1Char('\n') + line;
    }

    QProcess m_task;
    QProcess m_status;
    QString m_enrolment;
    QString m_trouble;
    QString m_pose;
    QString m_log;
    bool m_pamEnabled = false;
};

K_PLUGIN_CLASS_WITH_JSON(FaciolockKcm, "kcm_faciolock.json")

#include "faciolockkcm.moc"
