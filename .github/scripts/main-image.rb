require 'json'
require 'open3'
require 'yaml'

# main 镜像只能使用成功 CI 的源码；发布机器人只改 published 时可沿用原提交的 CI。
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
      endpoint = "repos/#{repository}/actions/workflows/ci.yml/runs?event=push&branch=main&head_sha=#{candidate}&per_page=1"
      run = JSON.parse(capture('gh', 'api', endpoint)).fetch('workflow_runs').first
      if run
        if run.fetch('head_sha') == candidate && run.fetch('head_repository').fetch('full_name') == repository &&
           run.fetch('conclusion') == 'success'
          return candidate
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
      sha = MainImage.resolve(ENV.fetch('REQUESTED_SHA'), ENV.fetch('MAIN_SHA'), ENV.fetch('GITHUB_REPOSITORY'))
      puts "sha=#{sha}"
    when ['current']
      # 构建期间 main 或同提交的最新 CI 结果都可能变化，提升前重新取得证据。
      source = ENV.fetch('SOURCE_SHA')
      current = MainImage.resolve(source, ENV.fetch('MAIN_SHA'), ENV.fetch('GITHUB_REPOSITORY')) == source
      puts "promote=#{current}"
    else
      raise 'usage: main-image.rb resolve|current'
    end
  rescue StandardError => error
    warn "error: #{error.message}"
    exit 1
  end
end
