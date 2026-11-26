require 'json'
require 'open3'
require 'yaml'
require_relative 'build-artifacts'

# main 只提升成功 CI 已验证的镜像；发布机器人只改 published 时可沿用原构建。
module MainImage
  VERSION_PATH = 'release/version.yaml'

  def self.capture(*command)
    output, error, status = Open3.capture3(*command)
    raise "#{command.first} failed: #{error.strip}" unless status.success?
    output.strip
  end

  def self.same_build?(source, current)
    return true if source == current
    _, error, status = Open3.capture3('git', 'merge-base', '--is-ancestor', source, current)
    return false if status.exitstatus == 1
    raise "Cannot verify main ancestry: #{error.strip}" unless status.success?
    paths = capture('git', 'diff', '--name-only', '--no-renames', source, current).lines.map(&:strip)
    return false unless paths == [VERSION_PATH]

    # published 不进入二进制；构建版本或任何源码变化都必须重新取得该提交的 CI 证据。
    documents = [source, current].map do |sha|
      YAML.safe_load(capture('git', 'show', "#{sha}:#{VERSION_PATH}")).reject { |key, _| key == 'published' }
    end
    documents.first == documents.last
  end

  def self.resolve(requested, current, repository)
    unless same_build?(requested, current)
      warn 'Main source changed; skipping this image request'
      return nil
    end

    candidate = requested
    loop do
      run = BuildArtifacts.ci_run(repository, candidate)
      if run
        if run.fetch('status') == 'completed' && run.fetch('conclusion') == 'success'
          artifact = BuildArtifacts.artifact(repository, run)
          warn "No retained verified build for #{candidate}; rerun CI to build it" unless artifact
          return artifact
        end
        # 失败或仍在运行的 CI 不能被旧的成功记录替代。
        warn "Latest CI is not successful for #{candidate}; skipping this image request"
        return nil
      end

      # GITHUB_TOKEN 写回索引不会触发 CI；仅穿过这种单父提交寻找原源码的成功记录。
      parents = capture('git', 'rev-list', '--parents', '-n', '1', candidate).split.drop(1)
      unless parents.length == 1 && same_build?(parents.first, candidate)
        warn "No successful CI for #{candidate}; skipping this image request"
        return nil
      end
      candidate = parents.first
    end
  end
end

if $PROGRAM_NAME == __FILE__
  begin
    case ARGV
    when ['resolve']
      build = MainImage.resolve(ENV.fetch('REQUESTED_SHA'), ENV.fetch('MAIN_SHA'), ENV.fetch('GITHUB_REPOSITORY'))
      %w[sha run_id artifact_id].each { |key| puts "#{key}=#{build&.fetch(key)}" }
    when ['current']
      # 下载期间 main 或 CI 重跑结果可能变化；旧 artifact 不能替代新一次验证产物。
      source = ENV.fetch('SOURCE_SHA')
      build = MainImage.resolve(source, ENV.fetch('MAIN_SHA'), ENV.fetch('GITHUB_REPOSITORY'))
      current = build && build.fetch('sha') == source && build.fetch('artifact_id').to_s == ENV.fetch('ARTIFACT_ID')
      puts "promote=#{!!current}"
    else
      raise 'usage: main-image.rb resolve|current'
    end
  rescue StandardError => error
    warn "error: #{error.message}"
    exit 1
  end
end
