require 'digest'
require_relative 'release-metadata'
require_relative 'toolchains'

# 发布只消费本仓库 main push 成功运行中的不可变 artifact ID，不按“最近成功”跨提交寻找。
module BuildArtifacts
  NAME = 'verified-build-linux-amd64'

  def self.capture(*command)
    output, error, status = Open3.capture3(*command)
    raise "#{command.first} failed: #{error.strip}" unless status.success?
    output.strip
  end

  def self.artifact(repository, run)
    rows = capture('gh', 'api', '--paginate', "repos/#{repository}/actions/runs/#{run.fetch('id')}/artifacts?per_page=100", '--jq', '.artifacts[] | @json')
    matches = rows.lines.map { |line| JSON.parse(line) }.select { |entry| entry.fetch('name') == NAME && !entry.fetch('expired') }
    raise 'Multiple verified build artifacts in one CI run' if matches.length > 1
    return nil if matches.empty?
    { 'sha' => run.fetch('head_sha'), 'run_id' => run.fetch('id'), 'artifact_id' => matches.first.fetch('id') }
  end

  def self.ci_run(repository, sha, wait: false)
    endpoint = "repos/#{repository}/actions/workflows/ci.yml/runs?event=push&branch=main&head_sha=#{sha}&per_page=1"
    run = JSON.parse(capture('gh', 'api', endpoint)).fetch('workflow_runs').first
    return nil unless run
    unless run.fetch('head_sha') == sha && run.fetch('event') == 'push' && run.fetch('head_branch') == 'main' &&
           run.fetch('head_repository').fetch('full_name') == repository
      raise 'CI source does not match the requested repository, commit and main push'
    end
    if wait && run.fetch('status') != 'completed'
      # tag 与 main push 可以同时触发；等已有 CI 完成，避免发行抢先重复编译同一提交。
      unless system('gh', 'run', 'watch', run.fetch('id').to_s, '--repo', repository, '--exit-status', '--interval', '30', out: $stderr)
        raise 'CI did not finish successfully; refusing to start a duplicate release build'
      end
      run = JSON.parse(capture('gh', 'api', "repos/#{repository}/actions/runs/#{run.fetch('id')}"))
      raise 'CI is still running after the wait' unless run.fetch('status') == 'completed'
    end
    run
  end

  def self.find(repository, sha, wait: false)
    run = ci_run(repository, sha, wait: wait)
    return nil unless run && run.fetch('status') == 'completed' && run.fetch('conclusion') == 'success'
    artifact(repository, run)
  end

  def self.checksums(directory)
    files = Dir.glob('{binary,frontend}/**/*', File::FNM_DOTMATCH, base: directory).reject { |path| %w[. ..].include?(File.basename(path)) }
    files.sort.each_with_object({}) do |path, result|
      full = File.join(directory, path)
      raise "Unexpected symlink in build artifact: #{path}" if File.symlink?(full)
      result[path] = Digest::SHA256.file(full).hexdigest if File.file?(full)
    end
  end

  def self.pack(directory)
    metadata = ReleaseMetadata.build.merge(
      'build_time' => ENV.fetch('CPR_BUILD_TIME'),
      'platform' => 'linux/amd64',
      'toolchains' => BuildToolchains.read,
      'run_id' => ENV.fetch('GITHUB_RUN_ID'),
      'image' => ENV.fetch('ARTIFACT_IMAGE'),
      'files' => checksums(directory)
    )
    File.write(File.join(directory, 'manifest.json'), JSON.pretty_generate(metadata) + "\n")
    verify(directory)
  end

  def self.verify(directory)
    metadata = JSON.parse(File.read(File.join(directory, 'manifest.json')))
    expected = ReleaseMetadata.build
    %w[version git_sha build_type].each do |key|
      raise "Artifact #{key} does not match this source" unless metadata.fetch(key) == expected.fetch(key)
    end
    raise 'Artifact platform does not match linux/amd64' unless metadata.fetch('platform') == 'linux/amd64'
    raise 'Artifact toolchains do not match this source' unless metadata.fetch('toolchains') == BuildToolchains.read
    raise 'Artifact CI run does not match' unless metadata.fetch('run_id') == ENV.fetch('BUILD_RUN_ID', ENV.fetch('GITHUB_RUN_ID'))
    Time.iso8601(metadata.fetch('build_time'))
    image_prefix = "ghcr.io/#{ENV.fetch('GITHUB_REPOSITORY').downcase}@sha256:"
    raise 'Invalid artifact image digest' unless metadata.fetch('image').match?(/\A#{Regexp.escape(image_prefix)}[a-f0-9]{64}\z/)
    actual = checksums(directory)
    unless actual.key?('binary/codex-proxy-rs') && actual.key?('frontend/index.html') && actual == metadata.fetch('files')
      raise 'Artifact files or SHA-256 checksums do not match'
    end
    metadata
  end
end

if $PROGRAM_NAME == __FILE__
  begin
    command, directory = ARGV
    case command
    when 'find'
      match = BuildArtifacts.find(ENV.fetch('GITHUB_REPOSITORY'), ReleaseMetadata.git('rev-parse', 'HEAD'), wait: true)
      puts "run_id=#{match&.fetch('run_id')}"
      puts "artifact_id=#{match&.fetch('artifact_id')}"
    when 'pack' then BuildArtifacts.pack(directory)
    when 'verify'
      metadata = BuildArtifacts.verify(directory)
      puts "image=#{metadata.fetch('image')}"
      puts "build_time=#{metadata.fetch('build_time')}"
    else raise 'usage: build-artifacts.rb find|pack DIRECTORY|verify DIRECTORY'
    end
  rescue StandardError => error
    warn "error: #{error.message}"
    exit 1
  end
end
